//! Unit tests ported from php-src `ext/sqlite3/tests/*.phpt`.
//!
//! Each test replays the phpt script step by step against an in-memory
//! database and compares the produced output (echoes, `var_dump()`s,
//! warnings) with the phpt expectation. Userland callbacks cannot run yet:
//! callbacks are native functions registered by the tests (or built-ins
//! such as `strtoupper`), with any state they need in a thread-local.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::value::float_fmt;
use php_runtime::{
    Args, ArrayKey, CallArgs, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, Value, Vm,
};

use super::capi;
use crate::testing::{TestVm, thrown};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// `assert_eq!` with PHP identity (`===`).
macro_rules! assert_v {
    ($a:expr, $b:expr $(,)?) => {{
        let (a, b): (Value, Value) = ($a, $b);
        assert!(Value::identical(&a, &b), "{a:?} !== {b:?}");
    }};
    ($a:expr, $b:expr, $($msg:tt)+) => {{
        let (a, b): (Value, Value) = ($a, $b);
        assert!(Value::identical(&a, &b), "{a:?} !== {b:?}: {}", format!($($msg)+));
    }};
}

fn s(x: &str) -> Value {
    Value::str(x)
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn b(x: bool) -> Value {
    Value::Bool(x)
}

fn var_dump_into(out: &mut String, v: &Value, indent: usize) {
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    match v {
        Value::Null | Value::Undef => out.push_str("NULL\n"),
        Value::Bool(x) => out.push_str(&format!("bool({x})\n")),
        Value::Int(n) => out.push_str(&format!("int({n})\n")),
        Value::Float(d) => out.push_str(&format!("float({})\n", float_fmt::repr(*d, -1))),
        Value::String(st) => {
            out.push_str(&format!(
                "string({}) \"{}\"\n",
                st.len(),
                String::from_utf8_lossy(st)
            ));
        }
        Value::Array(a) => {
            out.push_str(&format!("array({}) {{\n", a.len()));
            for (k, v) in a.iter() {
                match &k {
                    ArrayKey::Int(n) => out.push_str(&format!("{pad}  [{n}]=>\n")),
                    ArrayKey::Str(st) => out.push_str(&format!("{pad}  [\"{st}\"]=>\n")),
                }
                var_dump_into(out, v, indent + 2);
            }
            out.push_str(&pad);
            out.push_str("}\n");
        }
        Value::Object(o) => {
            out.push_str(&format!(
                "object({})#{} (0) {{\n{pad}}}\n",
                o.class().name,
                o.id()
            ));
        }
        Value::Resource(r) => {
            out.push_str(&format!(
                "resource({}) of type ({})\n",
                r.id(),
                r.type_name()
            ));
        }
        Value::Ref(r) => var_dump_into(out, &r.get(), 0),
    }
}

fn print_r(v: &Value) -> String {
    match v {
        Value::Array(a) => {
            let mut out = String::from("Array\n(\n");
            for (k, v) in a.iter() {
                let v = match v {
                    Value::String(st) => st.to_string(),
                    Value::Int(n) => n.to_string(),
                    other => format!("{other:?}"),
                };
                out.push_str(&format!("    [{}] => {v}\n", k.to_php_str()));
            }
            out.push_str(")\n");
            out
        }
        other => format!("{other:?}"),
    }
}

struct T {
    t: TestVm,
}

impl T {
    fn new() -> T {
        T { t: TestVm::new() }
    }

    fn vm(&mut self) -> &mut Vm {
        &mut self.t.vm
    }

    fn echo(&mut self, x: &str) {
        self.t.vm.echo(x.as_bytes()).unwrap();
    }

    fn dump(&mut self, v: &Value) {
        let mut out = String::new();
        var_dump_into(&mut out, v, 0);
        self.echo(&out);
    }

    /// Output so far (drained).
    fn out(&self) -> String {
        self.t.output()
    }

    fn try_new(&mut self, class: &str, args: Vec<Value>) -> PResult<ObjRef> {
        let ce = self.t.vm.find_class(class.as_bytes()).expect("class");
        let obj = self.t.vm.instantiate(&ce)?;
        self.t.vm.call_method(&obj, b"__construct", args)?;
        Ok(obj)
    }

    /// `new SQLite3($filename)`.
    fn open(&mut self, filename: &str) -> ObjRef {
        self.try_new("SQLite3", vec![s(filename)])
            .unwrap_or_else(|e| panic!("new SQLite3() threw {}", desc(&e)))
    }

    /// `new SQLite3(':memory:')` (new_db.inc).
    fn db(&mut self) -> ObjRef {
        self.open(":memory:")
    }

    fn try_m(&mut self, obj: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
        self.t.vm.call_method(obj, name.as_bytes(), args)
    }

    /// A method call that must not throw.
    fn m(&mut self, obj: &ObjRef, name: &str, args: Vec<Value>) -> Value {
        match self.try_m(obj, name, args) {
            Ok(v) => v,
            Err(e) => panic!("{name}() threw {}", desc(&e)),
        }
    }

    /// The object returned by a method call.
    fn mo(&mut self, obj: &ObjRef, name: &str, args: Vec<Value>) -> ObjRef {
        match self.m(obj, name, args) {
            Value::Object(o) => o,
            other => panic!("{name}() returned {other:?}"),
        }
    }

    fn try_static(&mut self, class: &str, name: &str, args: Vec<Value>) -> PResult<Value> {
        let ce = self.t.vm.find_class(class.as_bytes()).expect("class");
        let m = ce.find_method(name.as_bytes()).expect("method").clone();
        self.t.vm.call_function(
            &m,
            CallArgs {
                positional: args,
                scope: Some(ce.clone()),
                ..CallArgs::default()
            },
        )
    }

    fn f(&mut self, name: &str, args: Vec<Value>) -> Value {
        match self.t.call(name, args) {
            Ok(v) => v,
            Err(e) => panic!("{name}() threw {}", desc(&e)),
        }
    }

    /// `echo $e::class, ': ', $e->getMessage(), PHP_EOL;`
    fn echo_exception(&mut self, r: PResult<Value>) {
        let (class, msg) = thrown(r);
        self.echo(&format!("{class}: {msg}\n"));
    }

    fn exec(&mut self, db: &ObjRef, sql: &str) -> Value {
        self.m(db, "exec", vec![s(sql)])
    }

    /// `while ($row = $result->fetchArray($mode)) var_dump($row);`
    fn dump_rows(&mut self, result: &ObjRef, mode: i64) {
        loop {
            let row = self.m(result, "fetchArray", vec![i(mode)]);
            if matches!(row, Value::Bool(false)) {
                break;
            }
            self.dump(&row);
        }
    }

    /// Registers a native function usable as a callback.
    fn register(&mut self, nf: &'static NativeFunction) {
        self.t.vm.register_function(Rc::new(Function::native(nf)));
    }
}

fn desc(f: &Flow) -> String {
    match f {
        Flow::Exception(e) => format!("{}: {}", e.class().name, Vm::exception_message(e)),
        other => format!("{other:?}"),
    }
}

/// Replaces the `%d` placeholders of a phpt expectation with `values`.
fn fill(expect: &str, values: &[i64]) -> String {
    let mut out = String::new();
    let mut parts = expect.split("%d");
    out.push_str(parts.next().unwrap_or_default());
    for (n, part) in parts.enumerate() {
        out.push_str(&values[n].to_string());
        out.push_str(part);
    }
    out
}

fn warning(func: &str, msg: &str) -> String {
    format!("\nWarning: {func}(): {msg} in /t.php on line 3\n")
}

const TIMENOW: i64 = 1_700_000_000;

/// The table most phpt files create and fill.
fn create_test_table(t: &mut T, db: &ObjRef) {
    t.echo("Creating Table\n");
    let v = t.exec(db, "CREATE TABLE test (time INTEGER, id STRING)");
    t.dump(&v);
    t.echo("INSERT into table\n");
    for id in ["a", "b"] {
        let v = t.exec(
            db,
            &format!("INSERT INTO test (time, id) VALUES ({TIMENOW}, '{id}')"),
        );
        t.dump(&v);
    }
}

const CREATED_AND_INSERTED: &str =
    "Creating Table\nbool(true)\nINSERT into table\nbool(true)\nbool(true)\n";

const ROW_A: &str = "array(2) {\n  [0]=>\n  int(1700000000)\n  [1]=>\n  string(1) \"a\"\n}\n";
const ROW_B: &str = "array(2) {\n  [0]=>\n  int(1700000000)\n  [1]=>\n  string(1) \"b\"\n}\n";

// ---------------------------------------------------------------------------
// Opening and closing
// ---------------------------------------------------------------------------

#[test]
fn open_close() {
    // sqlite3_01_open.phpt
    let mut t = T::new();
    let db = t.db();
    t.dump(&Value::Object(db.clone()));
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    t.echo("Done\n");
    assert_eq!(
        t.out(),
        fill(
            "object(SQLite3)#%d (0) {\n}\nbool(true)\nDone\n",
            &[i64::from(db.id())]
        )
    );
    // Closing twice is fine.
    assert_v!(t.m(&db, "close", vec![]), b(true));
}

#[test]
fn constructor_arguments() {
    // sqlite3_02_open.phpt
    let mut t = T::new();
    let r = t.try_new("SQLite3", vec![]).map(Value::Object);
    t.echo_exception(r);
    // sensitive_parameter.phpt
    let r = t
        .try_new(
            "SQLite3",
            vec![s(":memory:"), s("not-an-int"), s("secret-key")],
        )
        .map(Value::Object);
    t.echo_exception(r);
    let ce = t.vm().find_class("SQLite3").unwrap();
    let db = t.vm().instantiate(&ce).unwrap();
    let r = t.try_m(
        &db,
        "open",
        vec![s(":memory:"), s("not-an-int"), s("secret-key")],
    );
    t.echo_exception(r);
    let r = t.try_new("SQLite3", vec![s("a\0b")]).map(Value::Object);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "ArgumentCountError: SQLite3::__construct() expects at least 1 argument, 0 given\n\
         TypeError: SQLite3::__construct(): Argument #2 ($flags) must be of type int, string given\n\
         TypeError: SQLite3::open(): Argument #2 ($flags) must be of type int, string given\n\
         ValueError: SQLite3::__construct(): Argument #1 ($filename) must not contain any null bytes\n"
    );
}

#[test]
fn reinitialise_and_uninitialised_objects() {
    // sqlite3_31_open.phpt
    let mut t = T::new();
    let db = t.db();
    let r = t.try_m(&db, "open", vec![s(":memory:")]);
    t.echo_exception(r);
    assert_eq!(t.out(), "Exception: Already initialised DB Object\n");

    // An object created without its constructor
    // (sqlite3_trampoline_createfunction_no_leak.phpt).
    let ce = t.vm().find_class("SQLite3").unwrap();
    let raw = t.vm().instantiate(&ce).unwrap();
    for (name, args) in [
        ("exec", vec![s("SELECT 1")]),
        ("createFunction", vec![s("strtoupper"), s("strtoupper")]),
        ("lastErrorCode", vec![]),
        ("lastErrorMsg", vec![]),
        ("changes", vec![]),
        ("setAuthorizer", vec![Value::Null]),
    ] {
        let r = t.try_m(&raw, name, args);
        assert_eq!(
            thrown(r),
            (
                "Error".to_string(),
                "The SQLite3 object has not been correctly initialised or is already closed"
                    .to_string()
            ),
            "{name}"
        );
    }
    // close() and enableExceptions() work on any object.
    assert_v!(t.m(&raw, "close", vec![]), b(true));
    // Re-opening a closed database works.
    t.m(&db, "close", vec![]);
    t.m(&db, "open", vec![s(":memory:")]);
    assert_v!(t.m(&db, "querySingle", vec![s("SELECT 42")]), i(42));
}

#[test]
fn open_empty_string_and_files() {
    // sqlite3_open_empty_string.phpt
    let mut t = T::new();
    let db = t.open("");
    assert_v!(t.m(&db, "querySingle", vec![s("SELECT 1")]), i(1));

    // sqlite3_01_open-mb.phpt: a file database with a multibyte name.
    let dir = std::env::temp_dir().join(format!("php-sqlite3-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("私はガラスを食べられます.db");
    let db = t.open(file.to_str().unwrap());
    t.exec(&db, "CREATE TABLE t (a)");
    assert_v!(t.m(&db, "close", vec![]), b(true));
    assert!(file.exists());
    std::fs::remove_file(&file).unwrap();

    // sqlite3_15_open_error.phpt (skipped for root, like the phpt).
    let unreadable = dir.join("unreadable.db");
    std::fs::write(&unreadable, b"").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o200)).unwrap();
    if std::fs::read(&unreadable).is_err() {
        let r = t
            .try_new("SQLite3", vec![s(unreadable.to_str().unwrap())])
            .map(Value::Object);
        let (class, msg) = thrown(r);
        assert_eq!(class, "Exception");
        assert_eq!(msg, "Unable to open database: unable to open database file");
    }
    std::fs::remove_file(&unreadable).unwrap();
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn open_basedir_restrictions() {
    // sqlite3_21_security.phpt
    let mut t = T::new();
    let dir = std::env::temp_dir().join(format!("php-sqlite3-obd-{}", std::process::id()));
    let inner = dir.join("tests");
    std::fs::create_dir_all(&inner).unwrap();
    let inner = std::fs::canonicalize(&inner).unwrap();
    let inner_s = inner.to_str().unwrap().to_string();
    t.vm().ini_set("open_basedir", inner_s.clone());

    let file = inner.join("x.db");
    let db = t.open(file.to_str().unwrap());
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    std::fs::remove_file(&file).unwrap();

    let bad = format!("{}/../bad.db", inner_s);
    let r = t.try_new("SQLite3", vec![s(&bad)]).map(Value::Object);
    let (class, msg) = thrown(r);
    let full = format!(
        "{}/bad.db",
        std::fs::canonicalize(&dir).unwrap().to_str().unwrap()
    );
    assert_eq!(class, "Exception");
    assert_eq!(msg, format!("open_basedir prohibits opening {full}"));
    assert_eq!(
        t.out(),
        format!(
            "bool(true)\n{}",
            warning(
                "SQLite3::__construct",
                &format!(
                    "open_basedir restriction in effect. File({full}) is not within the allowed path(s): ({inner_s})"
                )
            )
        )
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn close_finalizes_statements() {
    // bug47159.phpt
    let mut t = T::new();
    let db = t.db();
    let stmt = t.mo(&db, "prepare", vec![s("SELECT 1")]);
    let v = t.m(&stmt, "close", vec![]);
    t.dump(&v);
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    assert_eq!(t.out(), "bool(true)\nbool(true)\n");

    // bug66550.phpt
    let db = t.db();
    t.exec(&db, "CREATE TABLE foo (id INTEGER, bar STRING)");
    let stmt = t.mo(&db, "prepare", vec![s("SELECT bar FROM foo WHERE id=:id")]);
    t.m(&db, "close", vec![]);
    let r = t.try_m(&stmt, "reset", vec![]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "Error: The SQLite3 object has not been correctly initialised or is already closed\n"
    );

    // sqlite3_12_unfinalized_stmt_cleanup.phpt
    let db = t.db();
    create_test_table(&mut t, &db);
    t.echo("SELECTING results\n");
    let results = t.mo(&db, "query", vec![s("SELECT * FROM test ORDER BY id ASC")]);
    let row = t.m(&results, "fetchArray", vec![i(2)]);
    t.dump(&row);
    t.echo("Closing database\n");
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    t.echo("Check db was closed\n");
    let r = t.try_m(&results, "numColumns", vec![]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        format!(
            "{CREATED_AND_INSERTED}SELECTING results\n{ROW_A}Closing database\nbool(true)\nCheck db was closed\n\
             Error: The SQLite3Result object has not been correctly initialised or is already closed\n"
        )
    );
}

#[test]
fn version_and_escape_string() {
    // sqlite3_17_version.phpt
    let mut t = T::new();
    let v = t.try_static("SQLite3", "version", vec![]).unwrap();
    assert_eq!(
        print_r(&v),
        format!(
            "Array\n(\n    [versionString] => {}\n    [versionNumber] => {}\n)\n",
            capi::libversion(),
            capi::libversion_number()
        )
    );

    // gh_sqlite3_escapestring_nul.phpt
    for case in ["\0", "a\0b", "secret\0x", "foo\0\0bar"] {
        let r = t.try_static("SQLite3", "escapeString", vec![s(case)]);
        t.echo_exception(r);
    }
    let ok = t
        .try_static("SQLite3", "escapeString", vec![s("ok")])
        .unwrap();
    let quote = t
        .try_static("SQLite3", "escapeString", vec![s("test''%")])
        .unwrap();
    let empty = t
        .try_static("SQLite3", "escapeString", vec![s("")])
        .unwrap();
    assert_v!(ok, s("ok"));
    assert_v!(quote, s("test''''%"));
    assert_v!(empty, s(""));
    assert_eq!(
        t.out(),
        "ValueError: SQLite3::escapeString(): Argument #1 ($string) must not contain any null bytes\n"
            .repeat(4)
    );
}

// ---------------------------------------------------------------------------
// exec / query / querySingle
// ---------------------------------------------------------------------------

#[test]
fn insert_and_query() {
    // sqlite3_03_insert.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.echo("SELECTING results\n");
    let results = t.mo(&db, "query", vec![s("SELECT * FROM test ORDER BY id ASC")]);
    t.dump_rows(&results, 2);
    t.m(&results, "finalize", vec![]);
    t.echo("Closing database\n");
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    t.echo("Done\n");
    assert_eq!(
        t.out(),
        format!(
            "{CREATED_AND_INSERTED}SELECTING results\n{ROW_A}{ROW_B}Closing database\nbool(true)\nDone\n"
        )
    );
}

#[test]
fn update_delete_and_changes() {
    // sqlite3_04_update.phpt / sqlite3_05_delete.phpt / sqlite3_18_changes.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.echo("UPDATE query\n");
    let v = t.exec(&db, "UPDATE test SET id = 'c'");
    t.dump(&v);
    t.echo("Rows Updated\n");
    let v = t.m(&db, "changes", vec![]);
    t.dump(&v);
    t.out();
    assert_v!(t.exec(&db, "DELETE FROM test WHERE id = 'c'"), b(true));
    assert_v!(t.m(&db, "changes", vec![]), i(2));
    let r = t.mo(&db, "query", vec![s("SELECT * FROM test")]);
    assert_v!(t.m(&r, "fetchArray", vec![]), b(false));

    // sqlite3_31_changes.phpt
    let db = t.db();
    assert_v!(t.m(&db, "changes", vec![]), i(0));
}

#[test]
fn last_insert_row_id() {
    // sqlite3_24_last_insert_rowid.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (time INTEGER, id STRING)");
    t.exec(&db, "INSERT INTO test (time, id) VALUES (1, 'a')");
    assert_v!(t.m(&db, "lastInsertRowID", vec![]), i(1));
    t.exec(&db, "INSERT INTO test (time, id) VALUES (1, 'b')");
    assert_v!(t.m(&db, "lastInsertRowID", vec![]), i(2));
}

#[test]
fn query_single() {
    // sqlite3_14_querysingle.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.echo("SELECTING results\n");
    let v = t.m(
        &db,
        "querySingle",
        vec![s("SELECT id FROM test WHERE id = 'a'")],
    );
    t.dump(&v);
    let v = t.m(
        &db,
        "querySingle",
        vec![s("SELECT id, time FROM test WHERE id = 'a'"), b(true)],
    );
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{CREATED_AND_INSERTED}SELECTING results\nstring(1) \"a\"\narray(2) {{\n  [\"id\"]=>\n  string(1) \"a\"\n  [\"time\"]=>\n  int({TIMENOW})\n}}\n"
        )
    );
    // No row: NULL, or an empty array for the entire row.
    assert_v!(
        t.m(&db, "querySingle", vec![s("SELECT id FROM test WHERE 0")]),
        Value::Null
    );
    let v = t.m(
        &db,
        "querySingle",
        vec![s("SELECT id FROM test WHERE 0"), b(true)],
    );
    assert_eq!(v.as_array().map(|a| a.len()), Some(0));
    // Numeric column names become integer keys (add_assoc_zval).
    let v = t.m(&db, "querySingle", vec![s("SELECT 1 AS \"5\""), b(true)]);
    t.dump(&v);
    assert_eq!(t.out(), "array(1) {\n  [5]=>\n  int(1)\n}\n");
    // Empty SQL.
    assert_v!(t.m(&db, "querySingle", vec![s("")]), b(false));
    assert_v!(t.m(&db, "query", vec![s("")]), b(false));
}

#[test]
fn select_no_results() {
    // sqlite3_16_select_no_results.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (time INTEGER, id STRING)");
    let results = t.mo(&db, "query", vec![s("SELECT * FROM test ORDER BY id ASC")]);
    t.dump_rows(&results, 2);
    assert_v!(t.m(&results, "finalize", vec![]), b(true));
    assert_v!(t.m(&db, "close", vec![]), b(true));
    assert_eq!(t.out(), "");
}

#[test]
fn errors_and_error_codes() {
    // sqlite3_20_error.phpt
    let mut t = T::new();
    let db = t.db();
    t.echo("SELECTING from invalid table\n");
    let result = t.m(&db, "query", vec![s("SELECT * FROM non_existent_table")]);
    if matches!(result, Value::Bool(false)) {
        let code = t.m(&db, "lastErrorCode", vec![]);
        let msg = t.m(&db, "lastErrorMsg", vec![]);
        t.echo(&format!(
            "Error Code: {}\nError Msg: {}\n",
            code.as_int().unwrap(),
            msg.as_str().unwrap()
        ));
    }
    t.echo("Closing database\n");
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    // bug69972.phpt: error accessors of a closed database.
    let code = t.m(&db, "lastErrorCode", vec![]);
    let msg = t.m(&db, "lastErrorMsg", vec![]);
    let ext = t.m(&db, "lastExtendedErrorCode", vec![]);
    t.echo(&format!(
        "Error Code: {}\nError Msg: {}\n",
        code.as_int().unwrap(),
        msg.as_str().unwrap()
    ));
    assert_v!(ext, i(0));
    assert_v!(t.m(&db, "enableExtendedResultCodes", vec![]), b(false));
    assert_eq!(
        t.out(),
        format!(
            "SELECTING from invalid table\n{}Error Code: 1\nError Msg: no such table: non_existent_table\nClosing database\nbool(true)\nError Code: 0\nError Msg: \n",
            warning(
                "SQLite3::query",
                "Unable to prepare statement: no such table: non_existent_table"
            )
        )
    );
}

#[test]
fn extended_error_codes() {
    // sqlite3_38_extended_error.phpt + sqlite3_39_toggleExtended.phpt
    let mut t = T::new();
    let db = t.db();
    t.m(
        &db,
        "query",
        vec![s(
            "CREATE TABLE dog ( id INTEGER PRIMARY KEY, name TEXT, annoying INTEGER )",
        )],
    );
    t.m(
        &db,
        "query",
        vec![s("INSERT INTO dog VALUES (1, 'Annoying Dog', 1)")],
    );
    assert_v!(t.m(&db, "lastErrorCode", vec![]), i(0));
    t.m(
        &db,
        "query",
        vec![s("INSERT INTO dog VALUES (1, 'Annoying Dog', 1)")],
    );
    assert_v!(t.m(&db, "lastErrorCode", vec![]), i(19));
    assert_v!(t.m(&db, "lastExtendedErrorCode", vec![]), i(1555));
    assert_v!(
        t.m(&db, "enableExtendedResultCodes", vec![b(true)]),
        b(true)
    );
    t.m(
        &db,
        "query",
        vec![s("INSERT INTO DOG VALUES (1, 'Annoying Dog', 1)")],
    );
    assert_v!(t.m(&db, "lastErrorCode", vec![]), i(1555));
    let w = warning(
        "SQLite3::query",
        "Unable to execute statement: UNIQUE constraint failed: dog.id",
    );
    assert_eq!(t.out(), format!("{w}{w}"));
}

#[test]
fn enable_exceptions() {
    // sqlite3_enable_exceptions.phpt
    let mut t = T::new();
    let db = t.db();
    let v = t.m(&db, "enableExceptions", vec![b(true)]);
    t.dump(&v);
    let r = t.try_m(&db, "query", vec![s("SELECT * FROM non_existent_table")]);
    let Err(Flow::Exception(e)) = &r else {
        panic!("expected an exception")
    };
    assert_v!(e.get_prop(b"code").unwrap_or_default(), i(1));
    t.echo_exception(r);
    let v = t.m(&db, "enableExceptions", vec![b(false)]);
    t.dump(&v);
    t.m(&db, "query", vec![s("SELECT * FROM non_existent_table")]);
    t.echo("Closing database\n");
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "bool(false)\nSQLite3Exception: Unable to prepare statement: no such table: non_existent_table\n\
             \nDeprecated: SQLite3::enableExceptions(): Use of warnings for SQLite3 is deprecated in /t.php on line 3\n\
             bool(true)\n{}Closing database\nbool(true)\n",
            warning(
                "SQLite3::query",
                "Unable to prepare statement: no such table: non_existent_table"
            )
        )
    );
    // exec() reports SQLite's own message.
    t.m(&db, "open", vec![s(":memory:")]);
    t.m(&db, "enableExceptions", vec![b(true)]);
    let r = t.try_m(&db, "exec", vec![s("SELECT * FROM nope")]);
    assert_eq!(
        thrown(r),
        (
            "SQLite3Exception".to_string(),
            "no such table: nope".to_string()
        )
    );
}

#[test]
fn defensive_mode() {
    // sqlite3_defensive.phpt
    let mut t = T::new();
    let db = t.db();
    let v = t.exec(&db, "CREATE TABLE test (a, b);");
    t.dump(&v);
    let v = t.exec(&db, "PRAGMA writable_schema = ON;");
    t.dump(&v);
    let v = t.m(
        &db,
        "querySingle",
        vec![s("SELECT COUNT(*) FROM sqlite_master;")],
    );
    t.dump(&v);
    let v = t.m(&db, "querySingle", vec![s("DELETE FROM sqlite_master;")]);
    t.dump(&v);
    let v = t.m(
        &db,
        "querySingle",
        vec![s("SELECT COUNT(*) FROM sqlite_master;")],
    );
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "bool(true)\nbool(true)\nint(1)\n{}bool(false)\nint(1)\n",
            warning(
                "SQLite3::querySingle",
                "Unable to prepare statement: table sqlite_master may not be modified"
            )
        )
    );
}

#[test]
fn double_quoted_strings() {
    // sqlite3_dqs.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (s1 VARCHAR(255), s2 VARCHAR(255))");
    t.exec(&db, "INSERT INTO test VALUES ('test', \"test\")");
    let stmt = t.mo(&db, "prepare", vec![s("SELECT * FROM test")]);
    let r = t.mo(&stmt, "execute", vec![]);
    let row = t.m(&r, "fetchArray", vec![i(1)]);
    t.dump(&row);
    assert_eq!(
        t.out(),
        "array(2) {\n  [\"s1\"]=>\n  string(4) \"test\"\n  [\"s2\"]=>\n  string(4) \"test\"\n}\n"
    );
}

#[test]
fn busy_timeout() {
    // sqlite3_busyTimeout.phpt
    let mut t = T::new();
    let db = t.db();
    for ms in [0, -1000, 1000] {
        assert_v!(t.m(&db, "busyTimeout", vec![i(ms)]), b(true));
    }
}

// ---------------------------------------------------------------------------
// Prepared statements and binding
// ---------------------------------------------------------------------------

/// A `bindParam()` argument (`&$var`).
fn var(v: Value) -> PhpRef {
    PhpRef::new(v)
}

#[test]
fn prepared_statement_bound_variable() {
    // sqlite3_06_prepared_stmt.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.echo("SELECTING results\n");
    let stmt = t.mo(
        &db,
        "prepare",
        vec![s("SELECT * FROM test WHERE id = ? ORDER BY id ASC")],
    );
    let foo = var(s("c"));
    t.echo("BINDING Parameter\n");
    let v = t.m(
        &stmt,
        "bindParam",
        vec![i(1), Value::Ref(foo.clone()), i(3)],
    );
    t.dump(&v);
    foo.set(s("a"));
    let results = t.mo(&stmt, "execute", vec![]);
    t.dump_rows(&results, 2);
    t.m(&results, "finalize", vec![]);
    assert_eq!(
        t.out(),
        format!("{CREATED_AND_INSERTED}SELECTING results\nBINDING Parameter\nbool(true)\n{ROW_A}")
    );
}

#[test]
fn prepared_statement_bound_values_and_names() {
    // sqlite3_07_prepared_stmt.phpt, sqlite3_10_bound_value_name.phpt,
    // sqlite3_bound_value_at_name.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.out();
    for (sql, binds) in [
        (
            "SELECT * FROM test WHERE id = ? ORDER BY id ASC",
            vec![i(1)],
        ),
        (
            "SELECT * FROM test WHERE id = :id ORDER BY id ASC",
            vec![s(":id"), s("id")],
        ),
        (
            "SELECT * FROM test WHERE id = @id ORDER BY id ASC",
            vec![s("@id")],
        ),
    ] {
        let stmt = t.mo(&db, "prepare", vec![s(sql)]);
        for param in binds {
            let v = t.m(&stmt, "bindValue", vec![param, s("a"), i(3)]);
            t.dump(&v);
        }
        let results = t.mo(&stmt, "execute", vec![]);
        t.dump_rows(&results, 2);
        t.m(&results, "finalize", vec![]);
    }
    assert_eq!(
        t.out(),
        format!("bool(true)\n{ROW_A}bool(true)\nbool(true)\n{ROW_A}bool(true)\n{ROW_A}")
    );
}

#[test]
fn bind_param_null_variable() {
    // bug45798.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (time INTEGER, id STRING)");
    t.exec(&db, "INSERT INTO test (time, id) VALUES (1, 'a')");
    t.exec(&db, "INSERT INTO test (time, id) VALUES (1, 'b')");
    let stmt = t.mo(
        &db,
        "prepare",
        vec![s("SELECT * FROM test WHERE id = ? ORDER BY id ASC")],
    );
    let foo = var(Value::Null);
    t.m(&stmt, "bindParam", vec![i(1), Value::Ref(foo), i(3)]);
    let results = t.mo(&stmt, "execute", vec![]);
    t.dump_rows(&results, 2);
    t.m(&results, "finalize", vec![]);
    t.m(&db, "close", vec![]);
    assert_eq!(t.out(), "");
}

#[test]
fn bind_parameter_number_zero() {
    // bug72571.phpt
    let mut t = T::new();
    let db = t.db();
    let stmt = t.mo(&db, "prepare", vec![s("select 1 = ?")]);
    let v = t.m(&stmt, "bindParam", vec![i(0), Value::Ref(var(i(0)))]);
    assert_v!(v, b(false));
    let r = t.m(&stmt, "execute", vec![]);
    t.dump(&r);
    let id = r.as_object().unwrap().id();
    assert_eq!(t.out(), format!("object(SQLite3Result)#{id} (0) {{\n}}\n"));
    // Unknown names and numbers fail too.
    assert_v!(t.m(&stmt, "bindValue", vec![s("nope"), i(1)]), b(false));
    assert_v!(t.m(&stmt, "bindValue", vec![i(-1), i(1)]), b(false));
}

#[test]
fn bind_values_64bit() {
    // bug63921-64bit.phpt
    let mut t = T::new();
    let num = 100_004_313_234_244;
    let conn = t.db();
    t.m(
        &conn,
        "query",
        vec![s(
            "CREATE TABLE users (id INTEGER NOT NULL, num INTEGER NOT NULL, PRIMARY KEY(id))",
        )],
    );
    let stmt = t.mo(
        &conn,
        "prepare",
        vec![s("insert into users (id, num) values (:id, :num)")],
    );
    t.m(&stmt, "bindValue", vec![s(":id"), i(1), i(1)]);
    t.m(&stmt, "bindValue", vec![s(":num"), i(num), i(1)]);
    t.m(&stmt, "execute", vec![]);
    let r = t.mo(&conn, "query", vec![s("SELECT num FROM users")]);
    let row = t.m(&r, "fetchArray", vec![]);
    t.dump(&i(num));
    t.dump(&row.as_array().unwrap().get_int(0).unwrap().clone());
    assert_eq!(t.out(), "int(100004313234244)\nint(100004313234244)\n");

    // bug73333.phpt
    let db = t.db();
    t.exec(&db, "CREATE TABLE foo (bar INT)");
    for v in [i64::MIN, i64::MAX] {
        t.exec(&db, &format!("INSERT INTO foo VALUES ({v})"));
    }
    let res = t.mo(&db, "query", vec![s("SELECT bar FROM foo")]);
    for v in [i64::MIN, i64::MAX] {
        let row = t.m(&res, "fetchArray", vec![i(2)]);
        assert_v!(row.as_array().unwrap().get_int(0).unwrap().clone(), i(v));
    }
}

#[test]
fn bind_value_float_juggling() {
    // bug76665.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE foo (bar REAL)");
    let stmt = t.mo(&db, "prepare", vec![s("INSERT INTO foo VALUES (:bar)")]);
    t.m(&stmt, "bindValue", vec![s(":bar"), i(17), i(2)]);
    t.m(&stmt, "execute", vec![]);
    let v = t.m(&db, "querySingle", vec![s("SELECT bar FROM foo LIMIT 1")]);
    t.dump(&v);
    assert_eq!(t.out(), "float(17)\n");
}

#[test]
fn bind_type_inference() {
    // sqlite3_bind_bug68849.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(
        &db,
        "CREATE TABLE test (a INTEGER, b TEXT, c REAL);\
         INSERT INTO test VALUES (1, 'hello', 3.14);\
         INSERT INTO test VALUES (3, 'world', 3.15);\
         INSERT INTO test VALUES (0, '42', 0.42);",
    );
    for (sql, value) in [
        ("SELECT * FROM test WHERE (a+2) = ?;", i(3)),
        ("SELECT * FROM test WHERE a = ?;", b(true)),
        ("SELECT * FROM test WHERE a = ?;", b(false)),
        ("SELECT * FROM test WHERE c = ?;", Value::Float(3.15)),
    ] {
        let st = t.mo(&db, "prepare", vec![s(sql)]);
        t.m(&st, "bindValue", vec![i(1), value]);
        let r = t.mo(&st, "execute", vec![]);
        let row = t.m(&r, "fetchArray", vec![i(1)]);
        t.dump(&row);
    }
    let row = |a: i64, b: &str, c: &str| {
        format!(
            "array(3) {{\n  [\"a\"]=>\n  int({a})\n  [\"b\"]=>\n  string({}) \"{b}\"\n  [\"c\"]=>\n  float({c})\n}}\n",
            b.len()
        )
    };
    assert_eq!(
        t.out(),
        [
            row(1, "hello", "3.14"),
            row(1, "hello", "3.14"),
            row(0, "42", "0.42"),
            row(3, "world", "3.15"),
        ]
        .concat()
    );
}

#[test]
fn bind_param_conversions_stick() {
    // bug77051.phpt
    let mut t = T::new();
    let db = t.db();
    t.m(&db, "enableExceptions", vec![b(true)]);
    let stmt = t.mo(&db, "prepare", vec![s("SELECT :a, :b, ?;")]);
    let a = var(i(42));
    t.m(
        &stmt,
        "bindParam",
        vec![s(":a"), Value::Ref(a.clone()), i(1)],
    );
    t.m(&stmt, "bindValue", vec![s(":b"), s("php")]);
    t.m(&stmt, "bindValue", vec![s(":b"), s("PHP")]);
    t.m(&stmt, "bindValue", vec![i(3), i(424242)]);
    let base = i64::from(stmt.id());

    t.echo("Execute statement\n");
    let mut res = t.m(&stmt, "execute", vec![]);
    t.dump(&res);
    t.echo("Statement result\n");
    let row = t.m(res.as_object().unwrap(), "fetchArray", vec![i(2)]);
    t.dump(&row);

    t.echo("Change binded param to wrong type\n");
    a.set(s("TEST"));
    t.echo("Execute statement\n");
    res = t.m(&stmt, "execute", vec![]);
    t.dump(&res);
    t.echo("Statement result\n");
    let row = t.m(res.as_object().unwrap(), "fetchArray", vec![i(2)]);
    t.dump(&row);
    // convert_to_long() changed the bound variable.
    assert_v!(a.get(), i(0));

    t.echo("Change binded values\n");
    a.set(i(5_252_552));
    t.m(&stmt, "bindValue", vec![s(":b"), s("TEST")]);
    t.m(&stmt, "bindValue", vec![i(3), s("!!!")]);
    t.echo("Execute statement\n");
    res = t.m(&stmt, "execute", vec![]);
    t.dump(&res);
    t.echo("Statement result\n");
    let row = t.m(res.as_object().unwrap(), "fetchArray", vec![i(2)]);
    t.dump(&row);
    assert_eq!(
        t.out(),
        fill(
            "Execute statement\nobject(SQLite3Result)#%d (0) {\n}\nStatement result\n\
             array(3) {\n  [0]=>\n  int(42)\n  [1]=>\n  string(3) \"PHP\"\n  [2]=>\n  int(424242)\n}\n\
             Change binded param to wrong type\nExecute statement\nobject(SQLite3Result)#%d (0) {\n}\nStatement result\n\
             array(3) {\n  [0]=>\n  int(0)\n  [1]=>\n  string(3) \"PHP\"\n  [2]=>\n  int(424242)\n}\n\
             Change binded values\nExecute statement\nobject(SQLite3Result)#%d (0) {\n}\nStatement result\n\
             array(3) {\n  [0]=>\n  int(5252552)\n  [1]=>\n  string(4) \"TEST\"\n  [2]=>\n  string(3) \"!!!\"\n}\n",
            &[base + 1, base + 2, base + 1]
        )
    );
}

#[test]
fn bind_param_types() {
    // bug71049.phpt: binding as a blob does not touch the variable.
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (age INTEGER, id STRING)");
    let stmt = t.mo(
        &db,
        "prepare",
        vec![s("SELECT * FROM test WHERE id = ? ORDER BY id ASC")],
    );
    let foo = var(s("alive!"));
    t.m(
        &stmt,
        "bindParam",
        vec![i(1), Value::Ref(foo.clone()), i(4)],
    );
    t.m(&stmt, "execute", vec![]);
    t.dump(&foo.get());
    assert_eq!(t.out(), "string(6) \"alive!\"\n");

    // bindParam() without a type binds as text; NULL binds as NULL.
    let st = t.mo(
        &db,
        "prepare",
        vec![s("SELECT typeof(?), typeof(?), typeof(?)")],
    );
    t.m(&st, "bindParam", vec![i(1), Value::Ref(var(i(5)))]);
    t.m(&st, "bindValue", vec![i(2), i(5)]);
    t.m(&st, "bindValue", vec![i(3), s("x"), i(5)]);
    let r = t.mo(&st, "execute", vec![]);
    let row = t.m(&r, "fetchArray", vec![i(2)]);
    t.dump(&row);
    assert_eq!(
        t.out(),
        "array(3) {\n  [0]=>\n  string(4) \"text\"\n  [1]=>\n  string(7) \"integer\"\n  [2]=>\n  string(4) \"null\"\n}\n"
    );

    // An unknown type fails the execution.
    t.m(&st, "bindValue", vec![i(1), i(1), i(99)]);
    assert_v!(t.m(&st, "execute", vec![]), b(false));
    assert_eq!(
        t.out(),
        warning(
            "SQLite3Stmt::execute",
            "Unknown parameter type: 99 for parameter 1"
        )
    );
}

#[test]
fn clear_bindings() {
    // bug70628.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(
        &db,
        "CREATE TABLE Dogs (Id INTEGER PRIMARY KEY, Breed TEXT, Name TEXT, Age INTEGER)",
    );
    let sth = t.mo(
        &db,
        "prepare",
        vec![s(
            "INSERT INTO Dogs (Breed, Name, Age)  VALUES (:breed,:name,:age)",
        )],
    );
    t.m(&sth, "bindValue", vec![s(":breed"), s("canis"), i(3)]);
    t.m(&sth, "bindValue", vec![s(":name"), s("jack"), i(3)]);
    t.m(&sth, "bindValue", vec![s(":age"), i(7), i(1)]);
    t.m(&sth, "execute", vec![]);
    t.m(&sth, "clear", vec![]);
    t.m(&sth, "reset", vec![]);
    t.m(&sth, "bindValue", vec![s(":breed"), s("russel"), i(3)]);
    t.m(&sth, "bindValue", vec![s(":age"), i(3), i(1)]);
    t.m(&sth, "execute", vec![]);
    let res = t.mo(&db, "query", vec![s("SELECT * FROM Dogs")]);
    t.dump_rows(&res, 1);
    t.m(&res, "finalize", vec![]);
    t.m(&sth, "close", vec![]);
    t.m(&db, "close", vec![]);
    assert_eq!(
        t.out(),
        "array(4) {\n  [\"Id\"]=>\n  int(1)\n  [\"Breed\"]=>\n  string(5) \"canis\"\n  [\"Name\"]=>\n  string(4) \"jack\"\n  [\"Age\"]=>\n  int(7)\n}\n\
         array(4) {\n  [\"Id\"]=>\n  int(2)\n  [\"Breed\"]=>\n  string(6) \"russel\"\n  [\"Name\"]=>\n  NULL\n  [\"Age\"]=>\n  int(3)\n}\n"
    );
}

#[test]
fn reset_statements_and_results() {
    // sqlite3_26_reset_prepared_stmt.phpt, sqlite3_27_reset_prepared_stmt_result.phpt,
    // sqlite3_28_clear_bindings.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.out();
    let stmt = t.mo(
        &db,
        "prepare",
        vec![s("SELECT * FROM test WHERE id = ? ORDER BY id ASC")],
    );
    let foo = var(s("c"));
    t.m(
        &stmt,
        "bindParam",
        vec![i(1), Value::Ref(foo.clone()), i(3)],
    );
    foo.set(s("a"));
    let results = t.mo(&stmt, "execute", vec![]);
    t.dump_rows(&results, 2);
    t.m(&stmt, "reset", vec![]);
    t.dump_rows(&results, 2);
    t.m(&results, "reset", vec![]);
    t.dump_rows(&results, 2);
    t.m(&stmt, "reset", vec![]);
    t.m(&stmt, "clear", vec![]);
    let v = t.m(&stmt, "bindValue", vec![i(1), s("b"), i(3)]);
    t.dump(&v);
    let results = t.mo(&stmt, "execute", vec![]);
    t.dump_rows(&results, 2);
    t.m(&results, "finalize", vec![]);
    assert_eq!(t.out(), format!("{ROW_A}{ROW_A}{ROW_A}bool(true)\n{ROW_B}"));

    // sqlite3_33_reset.phpt
    let db = t.db();
    t.exec(&db, "CREATE TABLE foo (id INTEGER, bar STRING)");
    t.exec(
        &db,
        "INSERT INTO foo (id, bar) VALUES (1, 'This is a test')",
    );
    let stmt = t.mo(&db, "prepare", vec![s("SELECT bar FROM foo WHERE id=:id")]);
    t.m(&stmt, "bindValue", vec![s(":id"), i(1), i(1)]);
    assert_v!(t.m(&stmt, "reset", vec![]), b(true));
}

#[test]
fn column_type_after_reset() {
    // bug79294.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE foo (bar INT)");
    t.exec(&db, "INSERT INTO foo VALUES (1)");
    let stmt = t.mo(&db, "prepare", vec![s("SELECT * FROM foo")]);
    let res = t.mo(&stmt, "execute", vec![]);
    let step = |t: &mut T| {
        let row = t.m(&res, "fetchArray", vec![]);
        t.dump(&b(!matches!(row, Value::Bool(false))));
        let ty = t.m(&res, "columnType", vec![i(0)]);
        t.dump(&ty);
    };
    step(&mut t);
    step(&mut t);
    t.m(&stmt, "reset", vec![]);
    step(&mut t);
    t.m(&res, "reset", vec![]);
    step(&mut t);
    assert_eq!(
        t.out(),
        "bool(true)\nint(1)\nbool(false)\nbool(false)\nbool(true)\nint(1)\nbool(true)\nint(1)\n"
    );
}

#[test]
fn statement_introspection() {
    // sqlite3_35_stmt_readonly.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.out();
    for (sql, ro) in [
        ("SELECT * FROM test WHERE id = ? ORDER BY id ASC", true),
        ("UPDATE test SET id = 'c' WHERE id = ?", false),
        ("DELETE FROM test", false),
    ] {
        let stmt = t.mo(&db, "prepare", vec![s(sql)]);
        assert_v!(t.m(&stmt, "readOnly", vec![]), b(ro));
    }

    // sqlite3stmt_paramCount_basic.phpt (with this table's column names)
    for (sql, n) in [
        ("SELECT * FROM test WHERE id = ? ORDER BY id ASC", 1),
        ("SELECT * FROM test WHERE id = 2 ORDER BY id ASC", 0),
        (
            "SELECT * FROM test WHERE id = :id AND time = :time ORDER BY id ASC",
            2,
        ),
        (
            "SELECT * FROM test WHERE id = 1 AND time = :time ORDER BY id ASC",
            1,
        ),
    ] {
        let stmt = t.mo(&db, "prepare", vec![s(sql)]);
        assert_v!(t.m(&stmt, "paramCount", vec![]), i(n));
        t.m(&stmt, "execute", vec![]);
    }

    // sqlite3_stmt_busy.phpt
    t.exec(&db, "CREATE TABLE test_busy (a string);");
    t.exec(
        &db,
        "INSERT INTO test_busy VALUES ('interleaved'), ('statements')",
    );
    let st = t.mo(&db, "prepare", vec![s("SELECT a FROM test_busy")]);
    assert_v!(t.m(&st, "busy", vec![]), b(false));
    let r = t.mo(&st, "execute", vec![]);
    t.m(&r, "fetchArray", vec![]);
    assert_v!(t.m(&st, "busy", vec![]), b(true));
}

#[test]
fn get_sql() {
    // sqlite3stmt_getsql.phpt + sqlite3stmt_getsql_expanded.phpt
    let mut t = T::new();
    let db = t.db();
    t.m(&db, "enableExceptions", vec![b(true)]);
    let stmt = t.mo(&db, "prepare", vec![s("SELECT :a, :b, ?;")]);
    t.m(&stmt, "bindValue", vec![s(":a"), i(42)]);
    t.m(&stmt, "bindValue", vec![s(":b"), s("php")]);
    t.m(&stmt, "bindValue", vec![i(3), i(43)]);
    let v = t.m(&stmt, "getSQL", vec![b(false)]);
    t.dump(&v);
    let v = t.m(&stmt, "getSQL", vec![b(true)]);
    t.dump(&v);
    let res = t.m(&stmt, "execute", vec![]);
    t.dump(&res);
    let row = t.m(res.as_object().unwrap(), "fetchArray", vec![i(2)]);
    t.dump(&row);
    t.m(&stmt, "reset", vec![]);
    t.m(&stmt, "bindValue", vec![s(":a"), s("TEST")]);
    t.m(&stmt, "bindValue", vec![s(":b"), s("!!!")]);
    t.m(&stmt, "bindValue", vec![i(3), i(40)]);
    let v = t.m(&stmt, "getSQL", vec![b(true)]);
    t.dump(&v);
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        fill(
            "string(17) \"SELECT :a, :b, ?;\"\nstring(21) \"SELECT 42, 'php', 43;\"\n\
             object(SQLite3Result)#%d (0) {\n}\n\
             array(3) {\n  [0]=>\n  int(42)\n  [1]=>\n  string(3) \"php\"\n  [2]=>\n  int(43)\n}\n\
             string(25) \"SELECT 'TEST', '!!!', 40;\"\nbool(true)\n",
            &[i64::from(stmt.id()) + 1]
        )
    );
}

#[test]
fn text_with_nul_bytes() {
    // text_column_NUL_bytes.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE messages (\n        content TEXT\n    )");
    let insert = t.mo(
        &db,
        "prepare",
        vec![s("INSERT INTO messages (content) VALUES (:content)")],
    );
    t.m(
        &insert,
        "bindValue",
        vec![s(":content"), s("with\0null"), i(3)],
    );
    t.m(&insert, "execute", vec![]);
    t.m(&insert, "bindValue", vec![s(":content"), s("\0"), i(3)]);
    t.m(&insert, "execute", vec![]);
    let result = t.mo(&db, "query", vec![s("SELECT * FROM messages")]);
    t.dump_rows(&result, 1);
    assert_eq!(
        t.out(),
        "array(1) {\n  [\"content\"]=>\n  string(9) \"with\0null\"\n}\narray(1) {\n  [\"content\"]=>\n  string(1) \"\0\"\n}\n"
    );
}

#[test]
fn prepare_errors() {
    // sqlite3_prepare_faultystmt.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE foo (id INTEGER, bar STRING)");
    let v = t.m(&db, "prepare", vec![s("SELECT foo FROM bar")]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\n",
            warning(
                "SQLite3::prepare",
                "Unable to prepare statement: no such table: bar"
            )
        )
    );
    // sqlite3_prepare_with_empty_string.phpt
    assert_v!(t.m(&db, "prepare", vec![s("")]), b(false));
    // sqlite3_prepare_001.phpt: the statement outlives its local database variable.
    let stmt = {
        let class = t.db();
        t.mo(&class, "prepare", vec![s("SELECT 1")])
    };
    let r = t.mo(&stmt, "execute", vec![]);
    let v = t.m(&r, "fetchArray", vec![i(2)]);
    t.dump(&v);
    assert_eq!(t.out(), "array(1) {\n  [0]=>\n  int(1)\n}\n");

    // A statement without SQL (only a comment): the SQLite3Stmt checks fail.
    let empty = t.mo(&db, "prepare", vec![s("-- nothing")]);
    let r = t.try_m(&empty, "paramCount", vec![]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "Error: The SQLite3Stmt object has not been correctly initialised or is already closed\n"
    );
    let v = t.m(&empty, "execute", vec![]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\n",
            warning(
                "SQLite3Stmt::execute",
                "Unable to execute statement: out of memory"
            )
        )
    );
}

#[test]
fn statement_constructor() {
    // SQLite3Stmt::__construct() (private in the stub) prepares like prepare().
    let mut t = T::new();
    let db = t.db();
    let stmt = t
        .try_new(
            "SQLite3Stmt",
            vec![Value::Object(db.clone()), s("SELECT 7")],
        )
        .unwrap();
    let r = t.mo(&stmt, "execute", vec![]);
    let row = t.m(&r, "fetchArray", vec![i(2)]);
    t.dump(&row);
    assert_eq!(t.out(), "array(1) {\n  [0]=>\n  int(7)\n}\n");
    // Without a query the object stays uninitialised.
    let stmt = t
        .try_new("SQLite3Stmt", vec![Value::Object(db.clone()), s("")])
        .unwrap();
    let r = t.try_m(&stmt, "execute", vec![]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "Error: The SQLite3 object has not been correctly initialised or is already closed\n"
    );
    // Statements are finalized when the database is closed.
    let stmt = t.mo(&db, "prepare", vec![s("SELECT 1")]);
    t.m(&db, "close", vec![]);
    let r = t.try_m(&stmt, "execute", vec![]);
    t.echo_exception(r);
    let r = t.try_m(&stmt, "close", vec![]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "Error: The SQLite3 object has not been correctly initialised or is already closed\n"
            .repeat(2)
    );

    // SQLite3Result cannot be instantiated.
    let r = t.try_new("SQLite3Result", vec![]).map(Value::Object);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "Exception: SQLite3Result cannot be directly instantiated\n"
    );
}

#[test]
fn explain() {
    // sqlite3_explain.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test_explain (a string);");
    let insert = "INSERT INTO test_explain VALUES ('first insert'), ('second_insert')";
    let stmt = t.mo(&db, "prepare", vec![s(insert)]);
    assert_v!(t.m(&stmt, "setExplain", vec![i(1)]), b(true));
    assert_v!(t.m(&stmt, "explain", vec![]), i(1));
    let r = t.mo(&stmt, "execute", vec![]);
    let rows = t.m(&r, "fetchAll", vec![i(1)]);
    let rows = rows.as_array().unwrap();
    let first = rows.get_int(0).unwrap().as_array().unwrap().clone();
    let keys: Vec<String> = first.keys().map(|k| k.to_php_str().to_string()).collect();
    assert_eq!(
        keys,
        ["addr", "opcode", "p1", "p2", "p3", "p4", "p5", "comment"]
    );
    assert_v!(first.get_str(b"opcode").unwrap().clone(), s("Init"));
    let p4: Vec<Value> = rows
        .values()
        .map(|r| r.as_array().unwrap().get_str(b"p4").unwrap().clone())
        .collect();
    assert!(p4.iter().any(|v| Value::identical(v, &s("first insert"))));
    assert!(p4.iter().any(|v| Value::identical(v, &s("second_insert"))));

    let stmts = t.mo(&db, "prepare", vec![s("SELECT * FROM test_explain")]);
    t.m(&stmts, "setExplain", vec![i(2)]);
    let r = t.mo(&stmts, "execute", vec![]);
    let plan = t.m(&r, "fetchAll", vec![i(1)]);
    let plan = plan.as_array().unwrap().get_int(0).unwrap().clone();
    assert_v!(
        plan.as_array().unwrap().get_str(b"detail").unwrap().clone(),
        s("SCAN test_explain")
    );

    let stmt = t.mo(&db, "prepare", vec![s(insert)]);
    t.m(&stmt, "setExplain", vec![i(0)]);
    t.m(&stmt, "execute", vec![]);
    let stmts = t.mo(&db, "prepare", vec![s("SELECT * FROM test_explain")]);
    t.m(&stmts, "setExplain", vec![i(0)]);
    let r = t.mo(&stmts, "execute", vec![]);
    let all = t.m(&r, "fetchAll", vec![i(1)]);
    t.dump(&all);
    for mode in [-1, 256] {
        let r = t.try_m(&stmts, "setExplain", vec![i(mode)]);
        t.echo_exception(r);
    }
    assert_v!(t.m(&stmts, "explain", vec![]), i(0));
    assert_eq!(
        t.out(),
        "array(2) {\n  [0]=>\n  array(1) {\n    [\"a\"]=>\n    string(12) \"first insert\"\n  }\n  [1]=>\n  array(1) {\n    [\"a\"]=>\n    string(13) \"second_insert\"\n  }\n}\n\
         ValueError: SQLite3Stmt::setExplain(): Argument #1 ($mode) must be one of the SQLite3Stmt::EXPLAIN_MODE_* constants\n\
         ValueError: SQLite3Stmt::setExplain(): Argument #1 ($mode) must be one of the SQLite3Stmt::EXPLAIN_MODE_* constants\n"
    );
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

#[test]
fn fetch_modes() {
    // bug73068.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE IF NOT EXISTS t1(a INT UNIQUE, b INT)");
    t.exec(&db, "INSERT OR REPLACE INTO t1(a,b) VALUES('1','2')");
    let r = t.mo(
        &db,
        "query",
        vec![s("SELECT * FROM t1 WHERE a='1' AND b='2'")],
    );
    let row = t.m(&r, "fetchArray", vec![]);
    t.dump(&row);
    t.exec(&db, "DELETE FROM t1 WHERE a='1' AND b='2'");
    let r = t.mo(&db, "query", vec![s("SELECT * FROM t1;")]);
    let row = t.m(&r, "fetchArray", vec![]);
    t.dump(&row);
    assert_eq!(
        t.out(),
        "array(4) {\n  [0]=>\n  int(1)\n  [\"a\"]=>\n  int(1)\n  [1]=>\n  int(2)\n  [\"b\"]=>\n  int(2)\n}\nbool(false)\n"
    );
    // Neither flag: empty rows.
    let r = t.mo(&db, "query", vec![s("SELECT 1, 2")]);
    let row = t.m(&r, "fetchArray", vec![i(0)]);
    assert_eq!(row.as_array().map(|a| a.len()), Some(0));
}

#[test]
fn duplicate_column_names() {
    // gh11451.phpt
    let mut t = T::new();
    for sql in [
        "SELECT 1 AS key, 2 AS key",
        "SELECT 0 AS dummy, 1 AS key, 2 AS key",
    ] {
        let db = t.db();
        let r = t.mo(&db, "query", vec![s(sql)]);
        let row = t.m(&r, "fetchArray", vec![i(1)]);
        t.dump(&row);
    }
    assert_eq!(
        t.out(),
        "array(1) {\n  [\"key\"]=>\n  int(2)\n}\narray(2) {\n  [\"dummy\"]=>\n  int(0)\n  [\"key\"]=>\n  int(2)\n}\n"
    );
}

#[test]
fn column_info() {
    // sqlite3_19_columninfo.phpt
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.out();
    let result = t.mo(&db, "query", vec![s("SELECT * FROM test ORDER BY id ASC")]);
    while !matches!(t.m(&result, "fetchArray", vec![i(2)]), Value::Bool(false)) {
        let total = t.m(&result, "numColumns", vec![]).as_int().unwrap();
        for c in 0..total {
            let name = t.m(&result, "columnName", vec![i(c)]);
            let ty = t.m(&result, "columnType", vec![i(c)]);
            t.echo(&format!(
                "Name: {} - Type: {}\n",
                name.as_str().unwrap(),
                ty.as_int().unwrap()
            ));
        }
    }
    t.m(&result, "finalize", vec![]);
    assert_eq!(
        t.out(),
        "Name: time - Type: 1\nName: id - Type: 3\nName: time - Type: 1\nName: id - Type: 3\n"
    );

    // bug53463.phpt
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (whatever INTEGER)");
    t.exec(&db, "INSERT INTO test (whatever) VALUES (1)");
    let result = t.mo(&db, "query", vec![s("SELECT * FROM test")]);
    while !matches!(t.m(&result, "fetchArray", vec![i(2)]), Value::Bool(false)) {
        let v = t.m(&result, "columnName", vec![i(0)]);
        t.dump(&v);
        let v = t.m(&result, "columnName", vec![i(3)]);
        t.dump(&v);
    }
    // columnType() is false without a current row.
    assert_v!(t.m(&result, "columnType", vec![i(0)]), b(false));
    assert_eq!(t.out(), "string(8) \"whatever\"\nbool(false)\n");
}

#[test]
fn fetch_all() {
    // sqlite3_fetch_all.phpt
    let mut t = T::new();
    let conn = t.db();
    t.m(
        &conn,
        "query",
        vec![s(
            "CREATE TABLE users (id INTEGER NOT NULL, num INTEGER NOT NULL, PRIMARY KEY(id))",
        )],
    );
    t.m(
        &conn,
        "query",
        vec![s("insert into users (id, num) values (1, 1)")],
    );
    t.m(
        &conn,
        "query",
        vec![s("insert into users (id, num) values (2, 2)")],
    );
    let stmt = t.mo(&conn, "query", vec![s("SELECT * FROM users")]);
    let rowall = t.m(&stmt, "fetchAll", vec![]);
    t.dump(&rowall);
    t.m(&stmt, "reset", vec![]);
    let mut rowfetch = PhpArray::new();
    loop {
        let row = t.m(&stmt, "fetchArray", vec![]);
        if matches!(row, Value::Bool(false)) {
            break;
        }
        let _ = rowfetch.append(row);
    }
    let mut both = String::new();
    var_dump_into(&mut both, &Value::array(rowfetch), 0);
    t.m(&stmt, "reset", vec![]);
    let num = t.m(&stmt, "fetchAll", vec![i(2)]);
    t.dump(&num);
    t.m(&stmt, "reset", vec![]);
    let assoc = t.m(&stmt, "fetchAll", vec![i(1)]);
    t.dump(&assoc);
    let row_both = |n: i64| {
        format!(
            "  [{}]=>\n  array(4) {{\n    [0]=>\n    int({n})\n    [\"id\"]=>\n    int({n})\n    [1]=>\n    int({n})\n    [\"num\"]=>\n    int({n})\n  }}\n",
            n - 1
        )
    };
    let all_both = format!("array(2) {{\n{}{}}}\n", row_both(1), row_both(2));
    assert_eq!(both, all_both);
    assert_eq!(
        t.out(),
        format!(
            "{all_both}array(2) {{\n  [0]=>\n  array(2) {{\n    [0]=>\n    int(1)\n    [1]=>\n    int(1)\n  }}\n  [1]=>\n  array(2) {{\n    [0]=>\n    int(2)\n    [1]=>\n    int(2)\n  }}\n}}\n\
             array(2) {{\n  [0]=>\n  array(2) {{\n    [\"id\"]=>\n    int(1)\n    [\"num\"]=>\n    int(1)\n  }}\n  [1]=>\n  array(2) {{\n    [\"id\"]=>\n    int(2)\n    [\"num\"]=>\n    int(2)\n  }}\n}}\n"
        )
    );
}

#[test]
fn column_name_cache_and_rename() {
    // sqlite3_rename_column.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE tbl (orig text)");
    t.exec(&db, "insert into tbl values ('one'), ('two')");
    let s1 = t.mo(&db, "prepare", vec![s("select * from tbl")]);
    let res1 = t.mo(&s1, "execute", vec![]);
    let s2 = t.mo(&db, "prepare", vec![s("select * from tbl")]);
    let res2 = t.mo(&s2, "execute", vec![]);
    let first_key = |t: &mut T, r: &ObjRef| {
        let row = t.m(r, "fetchArray", vec![i(1)]);
        let k = row.as_array().unwrap().first().unwrap().0;
        k.to_php_str().to_string()
    };
    assert_eq!(first_key(&mut t, &res1), "orig");
    assert_eq!(first_key(&mut t, &res2), "orig");
    t.exec(&db, "alter table tbl rename column orig to changed");
    t.m(&res1, "reset", vec![]);
    assert_eq!(first_key(&mut t, &res1), "changed");
    assert_eq!(first_key(&mut t, &res2), "orig");
}

#[test]
fn fetch_errors() {
    // gh20699.phpt
    let mut t = T::new();
    let db = t.db();
    let st = t.mo(&db, "prepare", vec![s("BEGIN;")]);
    let r = t.mo(&st, "execute", vec![]);
    let v = t.m(&r, "fetchArray", vec![]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\n",
            warning(
                "SQLite3Result::fetchArray",
                "Unable to execute statement: cannot start a transaction within a transaction"
            )
        )
    );

    // gh22051.phpt
    let db = t.db();
    let sql = "SELECT abs(x) FROM (SELECT 1 AS x UNION ALL SELECT -9223372036854775807 - 1)";
    t.echo("--- SQLite3Result::reset() ---\n");
    let result = t.mo(&db, "query", vec![s(sql)]);
    let v = t.m(&result, "fetchArray", vec![i(2)]);
    t.dump(&v);
    let tok = t.vm().begin_silence();
    let v = t.m(&result, "fetchArray", vec![i(2)]);
    t.vm().end_silence(tok);
    t.dump(&v);
    let v = t.m(&result, "reset", vec![]);
    t.dump(&v);
    t.echo("--- SQLite3Result::finalize() ---\n");
    let stmt = t.mo(&db, "prepare", vec![s(sql)]);
    let result = t.mo(&stmt, "execute", vec![]);
    let v = t.m(&result, "fetchArray", vec![i(2)]);
    t.dump(&v);
    let tok = t.vm().begin_silence();
    let v = t.m(&result, "fetchArray", vec![i(2)]);
    t.vm().end_silence(tok);
    t.dump(&v);
    let v = t.m(&result, "finalize", vec![]);
    t.dump(&v);
    let one = "array(1) {\n  [0]=>\n  int(1)\n}\n";
    assert_eq!(
        t.out(),
        format!(
            "--- SQLite3Result::reset() ---\n{one}bool(false)\n{}bool(false)\n\
             --- SQLite3Result::finalize() ---\n{one}bool(false)\n{}bool(true)\n",
            warning(
                "SQLite3Result::reset",
                "Statement reset reported an error: integer overflow"
            ),
            warning(
                "SQLite3Result::finalize",
                "Statement reset reported an error: integer overflow"
            ),
        )
    );
}

#[test]
fn skip_all_cleanup() {
    // sqlite3_13_skip_all_cleanup.phpt: nothing is finalized or closed.
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    let results = t.mo(&db, "query", vec![s("SELECT * FROM test ORDER BY id ASC")]);
    t.dump_rows(&results, 2);
    t.echo("Done\n");
    assert_eq!(
        t.out(),
        format!("{CREATED_AND_INSERTED}{ROW_A}{ROW_B}Done\n")
    );
}

// ---------------------------------------------------------------------------
// Native callbacks standing in for the phpt files' userland functions
// ---------------------------------------------------------------------------

thread_local! {
    /// The `$db` a callback closes over.
    static CB_DB: RefCell<Option<ObjRef>> = const { RefCell::new(None) };
    /// The `$reported` flag of the close-from-callback phpt files.
    static REPORTED: Cell<bool> = const { Cell::new(false) };
}

fn cb_db() -> ObjRef {
    CB_DB
        .with(|d| d.borrow().clone())
        .expect("callback database")
}

fn set_cb_db(db: &ObjRef) {
    CB_DB.with(|d| *d.borrow_mut() = Some(db.clone()));
}

fn echo_dump(vm: &mut Vm, v: &Value) -> PResult<()> {
    let mut out = String::new();
    var_dump_into(&mut out, v, 0);
    vm.echo(out.as_bytes())
}

/// `try { var_dump($db->close()); } catch (Throwable $e) { echo ... }`
fn try_close(vm: &mut Vm) -> PResult<()> {
    let db = cb_db();
    match vm.call_method(&db, b"close", vec![]) {
        Ok(v) => echo_dump(vm, &v),
        Err(Flow::Exception(e)) => {
            let line = format!("{}: {}\n", e.class().name, Vm::exception_message(&e));
            vm.echo(line.as_bytes())
        }
        Err(f) => Err(f),
    }
}

fn first_time() -> bool {
    !REPORTED.with(|r| r.replace(true))
}

/// `echo "callback".PHP_EOL; throw new \Exception("oops");`
fn throw_oops(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo(b"callback\n")?;
    Err(vm.throw_error("Exception", "oops"))
}

/// `function my_udf_md5($string) { throw new \Exception("test exception\n"); }`
fn throw_test_exception(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "test exception\n"))
}

/// sqlite3_25_create_aggregate.phpt `sum_list_step()`.
fn sum_list_step(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mut context = match args.get(0) {
        Value::Array(a) if !a.is_empty() => (**a).clone(),
        _ => {
            let mut a = PhpArray::new();
            a.insert_str("total", 0i64);
            a.insert_str("values", Value::empty_array());
            a
        }
    };
    let add = vm.to_int(args.get(2))?;
    let total = context
        .get_str(b"total")
        .and_then(Value::as_int)
        .unwrap_or(0)
        + add;
    context.insert_str("total", total);
    let mut values = context
        .get_str(b"values")
        .and_then(|v| v.as_array().map(|a| (**a).clone()))
        .unwrap_or_default();
    let _ = values.append(total);
    context.insert_str("values", Value::array(values));
    Ok(Value::array(context))
}

/// sqlite3_25_create_aggregate.phpt `sum_list_finalize()`.
fn sum_list_finalize(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let values = match args.get(0) {
        Value::Array(a) => a.get_str(b"values").cloned().unwrap_or_default(),
        _ => Value::Null,
    };
    let parts: Vec<String> = values
        .as_array()
        .map(|a| {
            a.values()
                .map(|v| v.as_int().unwrap_or(0).to_string())
                .collect()
        })
        .unwrap_or_default();
    Ok(Value::from(parts.join(",")))
}

/// `function (int $action) { return $action == SQLite3::SELECT ? SQLite3::OK : SQLite3::DENY; }`
fn auth_select_only(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ok = args.get(0).as_int() == Some(i64::from(capi::ffi::SQLITE_SELECT));
    Ok(i(if ok { 0 } else { 1 }))
}

/// sqlite3_40_setauthorizer.phpt: dumps the action's constant name (the
/// last one with that value, like `array_flip()`) and the other arguments.
fn auth_dump(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let action = args.get(0).as_int().unwrap_or(-1);
    let ce = vm.find_class(b"SQLite3").expect("SQLite3");
    let mut name = String::new();
    for c in ce.constants.values() {
        if c.evaluated().and_then(|v| v.as_int()) == Some(action) {
            name = c.name.to_string();
        }
    }
    let rest: Vec<String> = args
        .rest(1)
        .iter()
        .map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_default())
        .collect();
    echo_dump(vm, &Value::from(name))?;
    echo_dump(vm, &Value::from(rest.join(",")))?;
    Ok(i(0))
}

fn return_fail(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(s("FAIL"))
}

fn return_4200(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(i(4200))
}

/// gh22122.phpt: an authorizer that removes itself.
fn auth_clear_self(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let db = cb_db();
    vm.call_method(&db, b"setAuthorizer", vec![Value::Null])?;
    vm.echo(b"method: alive\n")?;
    Ok(i(0))
}

/// sqlite3_close_from_callback.phpt UDF.
fn udf_close(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    try_close(vm)?;
    Ok(i(1))
}

/// sqlite3_close_from_callback_authorizer.phpt authorizer.
fn auth_close_once(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    if first_time() {
        try_close(vm)?;
    }
    Ok(i(0))
}

/// sqlite3_close_from_callback_aggregate.phpt step: `(int) $context + $a`.
fn agg_close_step(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    if first_time() {
        try_close(vm)?;
    }
    let ctx = vm.to_int(args.get(0))?;
    let a = vm.to_int(args.get(2))?;
    Ok(i(ctx + a))
}

fn agg_return_context(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(args.get(0).clone())
}

/// sqlite3_close_from_callback_collation.phpt: `strcmp($x, $y)`.
fn coll_close(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    if first_time() {
        try_close(vm)?;
    }
    let (x, y) = (
        vm.to_php_string(args.get(0))?,
        vm.to_php_string(args.get(1))?,
    );
    Ok(i(x.as_bytes().cmp(y.as_bytes()) as i64))
}

macro_rules! natives {
    ($($id:ident: $sig:literal => $handler:path;)*) => {
        $(static $id: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
    };
}

natives! {
    THROW_OOPS: "oopsFunction(mixed ...$args)" => throw_oops;
    THROW_TEST: "my_udf_throw(mixed ...$args)" => throw_test_exception;
    SUM_LIST_STEP: "sum_list_step(mixed $context, mixed $rows, mixed $string)" => sum_list_step;
    SUM_LIST_FINALIZE: "sum_list_finalize(mixed $context, mixed $rows)" => sum_list_finalize;
    AUTH_SELECT_ONLY: "auth_select_only(int $action, mixed ...$args)" => auth_select_only;
    AUTH_DUMP: "auth_dump(int $action, mixed ...$args)" => auth_dump;
    RETURN_FAIL: "return_fail(mixed ...$args)" => return_fail;
    RETURN_4200: "return_4200(mixed ...$args)" => return_4200;
    AUTH_CLEAR_SELF: "auth_clear_self(int $action, mixed ...$args)" => auth_clear_self;
    UDF_CLOSE: "udf_close(mixed ...$args)" => udf_close;
    AUTH_CLOSE_ONCE: "auth_close_once(mixed ...$args)" => auth_close_once;
    AGG_CLOSE_STEP: "agg_close_step(mixed $context, mixed $rows, mixed $a)" => agg_close_step;
    AGG_RETURN_CONTEXT: "agg_return_context(mixed $context, mixed $rows)" => agg_return_context;
    COLL_CLOSE: "coll_close(mixed $x, mixed $y)" => coll_close;
}

fn throws_to_string(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "Sorry"))
}

static THROWS_TOSTRING: NativeFunction = NativeFunction {
    sig: "public function __toString(): string",
    handler: throws_to_string,
};

/// exception_from_toString.phpt: `new throws` (`__toString()` throws).
fn throws_object(t: &mut T) -> ObjRef {
    let mut ce = ClassEntry::new("throws", 0);
    ce.add_method(Function::native_method(
        &THROWS_TOSTRING,
        "throws",
        member_flags::PUBLIC,
    ));
    ce.link_magic_methods();
    let ce = Rc::new(ce);
    t.vm().register_class(ce.clone());
    ObjRef::new(ce)
}

// ---------------------------------------------------------------------------
// User-defined functions, aggregates and collations
// ---------------------------------------------------------------------------

#[test]
fn create_function() {
    // sqlite3_08_udf.phpt (md5() as the UDF)
    let mut t = T::new();
    let db = t.db();
    create_test_table(&mut t, &db);
    t.out();
    let v = t.m(&db, "createFunction", vec![s("my_udf_md5"), s("md5")]);
    t.dump(&v);
    let results = t.mo(
        &db,
        "query",
        vec![s("SELECT my_udf_md5(id) FROM test ORDER BY id ASC")],
    );
    t.dump_rows(&results, 2);
    assert_eq!(
        t.out(),
        "bool(true)\narray(1) {\n  [0]=>\n  string(32) \"0cc175b9c0f1b6a831c399e269772661\"\n}\n\
         array(1) {\n  [0]=>\n  string(32) \"92eb5ffee6ae2fec3ad71c777531578f\"\n}\n"
    );

    // sqlite3_29_createfunction.phpt and sqlite3_37_createfunction_flags.phpt
    for extra in [vec![], vec![i(1), i(0x800)]] {
        let db = t.db();
        let args = |name: &str, f: &str| {
            let mut a = vec![s(name), s(f)];
            a.extend(extra.iter().cloned());
            a
        };
        let v = t.m(&db, "createFunction", args("strtoupper", "strtoupper"));
        t.dump(&v);
        let v = t.m(&db, "querySingle", vec![s("SELECT strtoupper('test')")]);
        t.dump(&v);
        let v = t.m(&db, "createFunction", args("strtolower", "strtolower"));
        t.dump(&v);
        let v = t.m(&db, "querySingle", vec![s("SELECT strtolower('TEST')")]);
        t.dump(&v);
        let v = t.m(&db, "createFunction", args("strtoupper", "strtolower"));
        t.dump(&v);
        let v = t.m(&db, "querySingle", vec![s("SELECT strtoupper('tEst')")]);
        t.dump(&v);
        assert_eq!(
            t.out(),
            "bool(true)\nstring(4) \"TEST\"\nbool(true)\nstring(4) \"test\"\nbool(true)\nstring(4) \"test\"\n"
        );
    }

    // sqlite3_trampoline_createfunction_no_leak.phpt
    let db = t.db();
    assert_v!(
        t.m(&db, "createFunction", vec![s(""), s("strtoupper")]),
        b(false)
    );
    let r = t.try_m(
        &db,
        "createFunction",
        vec![s("strtoupper"), s("strtoupper"), Value::Null, Value::Null],
    );
    assert!(r.is_ok());
    let r = t.try_m(&db, "createFunction", vec![s("f"), s("no_such_function")]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "\nDeprecated: SQLite3::createFunction(): Passing null to parameter #3 ($argCount) of type int is deprecated in /t.php on line 3\n\
         \nDeprecated: SQLite3::createFunction(): Passing null to parameter #4 ($flags) of type int is deprecated in /t.php on line 3\n\
         TypeError: SQLite3::createFunction(): Argument #2 ($callback) must be a valid callback, function \"no_such_function\" not found or invalid function name\n"
    );
}

#[test]
fn function_return_values() {
    // sqlite3_do_callback(): ints, floats, NULL and strings map to SQL types;
    // everything else is converted to a string.
    let mut t = T::new();
    let db = t.db();
    for (name, f) in [
        ("as_int", "intval"),
        ("as_float", "floatval"),
        ("as_bool", "boolval"),
        ("as_null", "is_null"),
    ] {
        t.m(&db, "createFunction", vec![s(name), s(f)]);
    }
    let v = t.m(
        &db,
        "querySingle",
        vec![
            s("SELECT typeof(as_int('5')), typeof(as_float('1.5')), as_bool(1), as_bool(0), typeof(as_null(NULL))"),
            b(true),
        ],
    );
    let vals: Vec<Value> = v.as_array().unwrap().values().cloned().collect();
    assert_v!(vals[0].clone(), s("integer"));
    assert_v!(vals[1].clone(), s("real"));
    assert_v!(vals[2].clone(), s("1"));
    assert_v!(vals[3].clone(), s(""));
    assert_v!(vals[4].clone(), s("text"));
    // Blobs reach the callback as strings.
    t.m(&db, "createFunction", vec![s("hex"), s("bin2hex")]);
    assert_v!(
        t.m(&db, "querySingle", vec![s("SELECT hex(x'00ff00')")]),
        s("00ff00")
    );
}

#[test]
fn create_aggregate() {
    // sqlite3_25_create_aggregate.phpt
    let mut t = T::new();
    t.register(&SUM_LIST_STEP);
    t.register(&SUM_LIST_FINALIZE);
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (a INTEGER, b INTEGER)");
    for (a, b_) in [(1, -1), (2, -2), (3, -3), (4, -4), (4, -4)] {
        t.exec(&db, &format!("INSERT INTO test (a, b) VALUES ({a}, {b_})"));
    }
    let v = t.m(
        &db,
        "createAggregate",
        vec![s("S"), s("sum_list_step"), s("sum_list_finalize"), i(1)],
    );
    assert_v!(v, b(true));
    let v = t.m(
        &db,
        "querySingle",
        vec![s("SELECT S(a), S(b) FROM test"), b(true)],
    );
    assert_eq!(
        print_r(&v),
        "Array\n(\n    [S(a)] => 1,3,6,10,14\n    [S(b)] => -1,-3,-6,-10,-14\n)\n"
    );
    // An aggregate over no rows: the final callback gets a NULL context.
    t.register(&AGG_RETURN_CONTEXT);
    t.m(
        &db,
        "createAggregate",
        vec![s("C"), s("sum_list_step"), s("agg_return_context")],
    );
    let v = t.m(&db, "querySingle", vec![s("SELECT C(a) FROM test WHERE 0")]);
    assert_v!(v, Value::Null);

    // sqlite3_33_createAggregate_notcallable.phpt
    let r = t.try_m(
        &db,
        "createAggregate",
        vec![
            s("TESTAGGREGATE"),
            s("aggregate_test_step"),
            s("sum_list_finalize"),
        ],
    );
    t.echo_exception(r);
    let r = t.try_m(
        &db,
        "createAggregate",
        vec![
            s("TESTAGGREGATE2"),
            s("sum_list_step"),
            s("aggregate_test_final"),
        ],
    );
    t.echo_exception(r);
    let v = t.m(
        &db,
        "createAggregate",
        vec![
            s("TESTAGGREGATE3"),
            s("sum_list_step"),
            s("sum_list_finalize"),
        ],
    );
    t.dump(&v);
    assert_eq!(
        t.out(),
        "TypeError: SQLite3::createAggregate(): Argument #2 ($stepCallback) must be a valid callback, function \"aggregate_test_step\" not found or invalid function name\n\
         TypeError: SQLite3::createAggregate(): Argument #3 ($finalCallback) must be a valid callback, function \"aggregate_test_final\" not found or invalid function name\n\
         bool(true)\n"
    );
    assert_v!(
        t.m(
            &db,
            "createAggregate",
            vec![s(""), s("sum_list_step"), s("sum_list_finalize")]
        ),
        b(false)
    );
}

#[test]
fn create_collation() {
    // sqlite3_36_create_collation.phpt
    let mut t = T::new();
    let db = t.db();
    t.m(&db, "createCollation", vec![s("NAT"), s("strnatcmp")]);
    t.exec(&db, "CREATE TABLE t (s varchar(4))");
    let stmt = t.mo(&db, "prepare", vec![s("INSERT INTO t VALUES (?)")]);
    for v in ["a1", "a10", "a2"] {
        t.m(&stmt, "bindParam", vec![i(1), Value::Ref(var(s(v)))]);
        t.m(&stmt, "execute", vec![]);
    }
    let default_sort = t.mo(&db, "query", vec![s("SELECT s FROM t ORDER BY s")]);
    let natural_sort = t.mo(
        &db,
        "query",
        vec![s("SELECT s FROM t ORDER BY s COLLATE NAT")],
    );
    for (label, r) in [("default", &default_sort), ("natural", &natural_sort)] {
        t.echo(&format!("{label}\n"));
        loop {
            let row = t.m(r, "fetchArray", vec![]);
            let Value::Array(a) = row else { break };
            let v = a.get_str(b"s").unwrap().as_str().unwrap().to_string();
            t.echo(&format!("{v}\n"));
        }
    }
    assert_eq!(t.out(), "default\na1\na10\na2\nnatural\na1\na2\na10\n");

    // A non-integer result is reported and compares as equal.
    t.register(&RETURN_FAIL);
    t.m(&db, "createCollation", vec![s("BAD"), s("return_fail")]);
    let r = t.mo(
        &db,
        "query",
        vec![s("SELECT s FROM t ORDER BY s COLLATE BAD")],
    );
    t.m(&r, "fetchArray", vec![]);
    let w = warning(
        "SQLite3::query",
        "An error occurred while invoking the compare callback (invalid return type).  Collation behaviour is undefined.",
    );
    assert!(t.out().starts_with(&w));
    assert_v!(
        t.m(&db, "createCollation", vec![s(""), s("strnatcmp")]),
        b(false)
    );
}

#[test]
fn callback_exceptions() {
    // bug68760.phpt: an exception from a collation.
    let mut t = T::new();
    t.register(&THROW_OOPS);
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (col1 string)");
    for v in ["a1", "a10", "a2"] {
        t.exec(&db, &format!("INSERT INTO test VALUES ('{v}')"));
    }
    t.m(
        &db,
        "createCollation",
        vec![s("NATURAL_CMP"), s("oopsFunction")],
    );
    let r = t.try_m(
        &db,
        "query",
        vec![s("SELECT col1 FROM test ORDER BY col1 COLLATE NATURAL_CMP")],
    );
    t.echo_exception(r);
    assert_eq!(t.out(), "callback\nException: oops\n");

    // bug72668.phpt: an exception from a function, with no extra warning.
    t.register(&THROW_TEST);
    let db = t.db();
    t.m(
        &db,
        "createFunction",
        vec![s("my_udf_md5"), s("my_udf_throw")],
    );
    let r = t.try_m(&db, "query", vec![s("SELECT my_udf_md5('test')")]);
    t.echo_exception(r);
    let r = t.try_m(&db, "querySingle", vec![s("SELECT my_udf_md5('test')")]);
    t.echo_exception(r);
    let statement = t.mo(&db, "prepare", vec![s("SELECT my_udf_md5('test')")]);
    let r = t.try_m(&statement, "execute", vec![]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "Exception: test exception\n\nException: test exception\n\nException: test exception\n\n"
    );
    // The connection is usable afterwards.
    assert_v!(t.m(&db, "querySingle", vec![s("SELECT 1")]), i(1));

    // An exception from an aggregate step stops later callbacks.
    t.m(
        &db,
        "createAggregate",
        vec![s("agg"), s("my_udf_throw"), s("strtoupper")],
    );
    let r = t.try_m(
        &db,
        "querySingle",
        vec![s("SELECT agg(x) FROM (SELECT 1 x UNION SELECT 2)")],
    );
    assert_eq!(thrown(r).1, "test exception\n");

    // With exceptions enabled, a later SQLite3Exception gets the callback's
    // exception as previous (fetchArray() reports even with one pending).
    t.m(&db, "enableExceptions", vec![b(true)]);
    let st = t.mo(
        &db,
        "prepare",
        vec![s("SELECT 1 UNION ALL SELECT my_udf_md5(1)")],
    );
    let res = t.mo(&st, "execute", vec![]);
    let row = t.m(&res, "fetchArray", vec![i(2)]);
    t.dump(&row);
    let r = t.try_m(&res, "fetchArray", vec![i(2)]);
    let Err(Flow::Exception(e)) = r else {
        panic!("expected exception")
    };
    assert_eq!(e.class().name.to_string(), "SQLite3Exception");
    assert!(Vm::exception_message(&e).starts_with("Unable to execute statement: "));
    let Some(Value::Object(prev)) = e.get_prop(b"previous") else {
        panic!("no previous exception")
    };
    assert_eq!(Vm::exception_message(&prev), "test exception\n");
    assert_eq!(t.out(), "array(1) {\n  [0]=>\n  int(1)\n}\n");
}

#[test]
fn exception_from_to_string() {
    // exception_from_toString.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE t(id int, v varchar(255))");
    let stmt = t.mo(&db, "prepare", vec![s("INSERT INTO t VALUES(:i, :v)")]);
    t.m(&stmt, "bindValue", vec![s("i"), i(1234)]);
    let obj = throws_object(&mut t);
    t.m(&stmt, "bindValue", vec![s("v"), Value::Object(obj)]);
    for _ in 0..2 {
        let r = t.try_m(&stmt, "execute", vec![]);
        if let Err(Flow::Exception(_)) = r {
            t.echo("Exception thrown ...\n");
        }
    }
    let query = t.mo(&db, "query", vec![s("SELECT * FROM t")]);
    t.dump_rows(&query, 1);
    assert_eq!(t.out(), "Exception thrown ...\nException thrown ...\n");
}

// ---------------------------------------------------------------------------
// Authorizer
// ---------------------------------------------------------------------------

#[test]
fn set_authorizer() {
    // sqlite3_40_setauthorizer.phpt
    let mut t = T::new();
    for nf in [&AUTH_SELECT_ONLY, &AUTH_DUMP, &RETURN_FAIL, &RETURN_4200] {
        t.register(nf);
    }
    let db = t.db();
    t.m(&db, "enableExceptions", vec![b(true)]);
    t.m(&db, "setAuthorizer", vec![s("auth_select_only")]);
    let v = t.m(&db, "querySingle", vec![s("SELECT 1;")]);
    t.dump(&v);
    let r = t.try_m(&db, "querySingle", vec![s("CREATE TABLE test (a, b);")]);
    t.echo_exception(r);
    t.m(&db, "setAuthorizer", vec![Value::Null]);
    let v = t.exec(&db, "CREATE TABLE test (a); INSERT INTO test VALUES (42);");
    t.dump(&v);
    let v = t.m(&db, "querySingle", vec![s("SELECT a FROM test;")]);
    t.dump(&v);
    t.m(&db, "setAuthorizer", vec![s("auth_dump")]);
    let v = t.exec(&db, "SELECT * FROM test WHERE a = 42;");
    t.dump(&v);
    let v = t.exec(&db, "DROP TABLE test;");
    t.dump(&v);
    for cb in ["return_fail", "return_4200"] {
        t.m(&db, "setAuthorizer", vec![s(cb)]);
        let r = t.try_m(&db, "querySingle", vec![s("SELECT 1;")]);
        let Err(Flow::Exception(e)) = &r else {
            panic!("expected exception")
        };
        let prev = match e.get_prop(b"previous") {
            Some(Value::Object(p)) => Vm::exception_message(&p),
            other => panic!("no previous exception: {other:?}"),
        };
        t.echo_exception(r);
        t.echo(&format!("{prev}\n"));
    }
    let pair = |action: &str, args: &str| {
        format!(
            "string({}) \"{action}\"\nstring({}) \"{args}\"\n",
            action.len(),
            args.len()
        )
    };
    let expected = [
        "int(1)\nSQLite3Exception: Unable to prepare statement: not authorized\nbool(true)\nint(42)\n".to_string(),
        pair("SELECT", ",,,"),
        pair("READ", "test,a,main,"),
        pair("READ", "test,a,main,"),
        "bool(true)\n".to_string(),
        pair("DELETE", "sqlite_master,,main,"),
        pair("DROP_TABLE", "test,,main,"),
        pair("DELETE", "test,,main,"),
        pair("DELETE", "sqlite_master,,main,"),
        pair("READ", "sqlite_master,tbl_name,main,"),
        pair("READ", "sqlite_master,type,main,"),
        pair("UPDATE", "sqlite_master,rootpage,main,"),
        pair("READ", "sqlite_master,rootpage,main,"),
        "bool(true)\n\
         SQLite3Exception: Unable to prepare statement: not authorized\n\
         The authorizer callback returned an invalid type: expected int\n\
         SQLite3Exception: Unable to prepare statement: not authorized\n\
         The authorizer callback returned an invalid value: 4200\n"
            .to_string(),
    ]
    .concat();
    assert_eq!(t.out(), expected);

    // sqlite3_trampoline_setauthorizer_null.phpt
    let db = t.db();
    t.m(&db, "enableExceptions", vec![b(true)]);
    t.m(&db, "setAuthorizer", vec![Value::Null]);
    assert_v!(t.m(&db, "querySingle", vec![s("SELECT 1;")]), i(1));
    assert_v!(
        t.m(&db, "querySingle", vec![s("CREATE TABLE test (a, b);")]),
        Value::Null
    );

    // Without exceptions the authorizer errors are warnings.
    let db = t.db();
    t.m(&db, "setAuthorizer", vec![s("return_fail")]);
    let v = t.m(&db, "querySingle", vec![s("SELECT 1;")]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}{}bool(false)\n",
            warning(
                "SQLite3::querySingle",
                "The authorizer callback returned an invalid type: expected int"
            ),
            warning(
                "SQLite3::querySingle",
                "Unable to prepare statement: not authorized"
            )
        )
    );
}

#[test]
fn authorizer_exceptions() {
    // A throwing authorizer: the warning is still emitted and the
    // callback's exception propagates.
    let mut t = T::new();
    t.register(&THROW_TEST);
    let db = t.db();
    t.m(&db, "setAuthorizer", vec![s("my_udf_throw")]);
    let r = t.try_m(&db, "querySingle", vec![s("SELECT 1")]);
    assert_eq!(thrown(r).1, "test exception\n");
    assert_eq!(
        t.out(),
        format!(
            "{}{}",
            warning(
                "SQLite3::querySingle",
                "An error occurred while invoking the authorizer callback"
            ),
            warning(
                "SQLite3::querySingle",
                "Unable to prepare statement: not authorized"
            )
        )
    );
    // With exceptions: SQLite3Exception -> SQLite3Exception -> Exception.
    t.m(&db, "enableExceptions", vec![b(true)]);
    let r = t.try_m(&db, "querySingle", vec![s("SELECT 1")]);
    let Err(Flow::Exception(e)) = r else {
        panic!("expected exception")
    };
    let mut chain = vec![];
    let mut cur = Some(e);
    while let Some(ex) = cur {
        chain.push(format!(
            "{}: {}",
            ex.class().name,
            Vm::exception_message(&ex)
        ));
        cur = match ex.get_prop(b"previous") {
            Some(Value::Object(p)) => Some(p),
            _ => None,
        };
    }
    assert_eq!(
        chain,
        [
            "SQLite3Exception: Unable to prepare statement: not authorized",
            "SQLite3Exception: An error occurred while invoking the authorizer callback",
            "Exception: test exception\n",
        ]
    );
}

#[test]
fn authorizer_replaced_during_callback() {
    // gh22122.phpt
    let mut t = T::new();
    t.register(&AUTH_CLEAR_SELF);
    let db = t.db();
    set_cb_db(&db);
    t.m(&db, "setAuthorizer", vec![s("auth_clear_self")]);
    t.exec(&db, "SELECT 1");
    t.m(&db, "setAuthorizer", vec![s("auth_clear_self")]);
    t.exec(&db, "SELECT 2");
    t.exec(&db, "SELECT 3");
    t.echo("post-disable query ok\n");
    assert_eq!(
        t.out(),
        "method: alive\nmethod: alive\npost-disable query ok\n"
    );
}

#[test]
fn authorizer_open_basedir() {
    // gh9032.phpt
    let mut t = T::new();
    t.vm().ini_set("open_basedir", ".");
    let db = t.db();
    t.m(&db, "enableExceptions", vec![b(true)]);
    t.exec(&db, "attach database ':memory:' AS \"db1\"");
    let v = t.exec(&db, "create table db1.r (id int)");
    t.dump(&v);
    let r = t.try_m(&db, "prepare", vec![s("attach database :a AS \"db2\"")]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "bool(true)\nSQLite3Exception: Unable to prepare statement: not authorized\n"
    );

    // bug81742.phpt: URIs are refused. php-src runs query() through
    // sqlite3_exec() when the result is unused; exec() gives that message.
    let db = t.db();
    t.exec(&db, "ATTACH 'file:..%2ffoo.php' as db2;");
    t.m(&db, "query", vec![s("ATTACH 'file:..%2ffoo.php' as db2;")]);
    assert_eq!(
        t.out(),
        format!(
            "{}{}",
            warning("SQLite3::exec", "not authorized"),
            warning(
                "SQLite3::query",
                "Unable to prepare statement: not authorized"
            )
        )
    );
    // Paths outside open_basedir are refused with the open_basedir warning.
    t.exec(&db, "ATTACH '/nonexistent-dir/x.db' as db3;");
    let out = t.out();
    assert!(
        out.contains("Warning: SQLite3::exec(): open_basedir restriction in effect. File(/nonexistent-dir/x.db)"),
        "{out}"
    );
    assert!(
        out.ends_with(&warning("SQLite3::exec", "not authorized")),
        "{out}"
    );
}

// ---------------------------------------------------------------------------
// close() from inside callbacks
// ---------------------------------------------------------------------------

const CANNOT_CLOSE: &str = "Error: Cannot close SQLite3 database while inside a callback\n";

#[test]
fn close_from_function_callback() {
    // sqlite3_close_from_callback.phpt
    let mut t = T::new();
    t.register(&UDF_CLOSE);
    let db = t.db();
    set_cb_db(&db);
    t.m(&db, "createFunction", vec![s("boom"), s("udf_close")]);
    let stmt = t.mo(&db, "prepare", vec![s("SELECT boom()")]);
    t.dump(&b(true));
    let res = t.mo(&stmt, "execute", vec![]);
    t.dump(&b(true));
    let v = t.m(&res, "fetchArray", vec![i(2)]);
    t.dump(&v);
    t.m(&res, "finalize", vec![]);
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "bool(true)\n{CANNOT_CLOSE}bool(true)\n{CANNOT_CLOSE}array(1) {{\n  [0]=>\n  int(1)\n}}\nbool(true)\n"
        )
    );
}

#[test]
fn close_from_aggregate_callback() {
    // sqlite3_close_from_callback_aggregate.phpt
    let mut t = T::new();
    t.register(&AGG_CLOSE_STEP);
    t.register(&AGG_RETURN_CONTEXT);
    let db = t.db();
    set_cb_db(&db);
    t.exec(&db, "CREATE TABLE t (a INTEGER)");
    t.exec(&db, "INSERT INTO t VALUES (1)");
    t.exec(&db, "INSERT INTO t VALUES (2)");
    t.m(
        &db,
        "createAggregate",
        vec![s("agg"), s("agg_close_step"), s("agg_return_context"), i(1)],
    );
    let stmt = t.mo(&db, "prepare", vec![s("SELECT agg(a) FROM t")]);
    let res = t.mo(&stmt, "execute", vec![]);
    let v = t.m(&res, "fetchArray", vec![i(2)]);
    t.dump(&v);
    t.m(&res, "finalize", vec![]);
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!("{CANNOT_CLOSE}array(1) {{\n  [0]=>\n  int(3)\n}}\nbool(true)\n")
    );
}

#[test]
fn close_from_collation_callback() {
    // sqlite3_close_from_callback_collation.phpt
    let mut t = T::new();
    t.register(&COLL_CLOSE);
    let db = t.db();
    set_cb_db(&db);
    t.exec(&db, "CREATE TABLE t (s TEXT)");
    t.exec(&db, "INSERT INTO t VALUES ('b')");
    t.exec(&db, "INSERT INTO t VALUES ('a')");
    t.m(&db, "createCollation", vec![s("CB"), s("coll_close")]);
    let stmt = t.mo(
        &db,
        "prepare",
        vec![s("SELECT s FROM t ORDER BY s COLLATE CB")],
    );
    let res = t.mo(&stmt, "execute", vec![]);
    loop {
        let row = t.m(&res, "fetchArray", vec![i(2)]);
        let Value::Array(a) = row else { break };
        let v = a.get_int(0).unwrap().as_str().unwrap().to_string();
        t.echo(&format!("{v}\n"));
    }
    t.m(&res, "finalize", vec![]);
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    assert_eq!(t.out(), format!("{CANNOT_CLOSE}a\nb\nbool(true)\n"));
}

#[test]
fn close_from_authorizer_callback() {
    // sqlite3_close_from_callback_authorizer.phpt
    let mut t = T::new();
    t.register(&AUTH_CLOSE_ONCE);
    let db = t.db();
    set_cb_db(&db);
    t.exec(&db, "CREATE TABLE t (a INTEGER)");
    t.exec(&db, "INSERT INTO t VALUES (7)");
    let live = t.mo(&db, "prepare", vec![s("SELECT a FROM t")]);
    t.m(&db, "setAuthorizer", vec![s("auth_close_once")]);
    t.m(&db, "prepare", vec![s("SELECT 1")]);
    let res = t.mo(&live, "execute", vec![]);
    let v = t.m(&res, "fetchArray", vec![i(2)]);
    t.dump(&v);
    t.m(&res, "finalize", vec![]);
    assert_eq!(
        t.out(),
        format!("{CANNOT_CLOSE}array(1) {{\n  [0]=>\n  int(7)\n}}\n")
    );
}

// ---------------------------------------------------------------------------
// Blobs
// ---------------------------------------------------------------------------

#[test]
fn open_blob_streams() {
    // sqlite3_30_blobopen.phpt
    let mut t = T::new();
    let db = t.db();
    t.echo("Creating Table\n");
    let v = t.exec(&db, "CREATE TABLE test (id STRING, data BLOB)");
    t.dump(&v);
    t.echo("PREPARING insert\n");
    let insert = t.mo(
        &db,
        "prepare",
        vec![s("INSERT INTO test (id, data) VALUES (?, ?)")],
    );
    t.echo("BINDING Parameter\n");
    let v = t.m(&insert, "bindValue", vec![i(1), s("a"), i(3)]);
    t.dump(&v);
    let v = t.m(&insert, "bindValue", vec![i(2), s("TEST TEST"), i(4)]);
    t.dump(&v);
    t.m(&insert, "execute", vec![]);
    t.echo("Closing statement\n");
    let v = t.m(&insert, "close", vec![]);
    t.dump(&v);
    let stream = t.m(&db, "openBlob", vec![s("test"), s("data"), i(1)]);
    t.dump(&stream);
    t.echo("Stream Contents\n");
    let v = t.f("stream_get_contents", vec![stream.clone()]);
    t.dump(&v);
    t.echo("Writing to read-only stream\n");
    let v = t.f("fwrite", vec![stream.clone(), s("ABCD")]);
    t.dump(&v);
    t.echo("Closing Stream\n");
    let v = t.f("fclose", vec![stream.clone()]);
    t.dump(&v);
    t.echo("Opening stream in write mode\n");
    let stream2 = t.m(
        &db,
        "openBlob",
        vec![s("test"), s("data"), i(1), s("main"), i(2)],
    );
    t.dump(&stream2);
    t.echo("Writing to blob\n");
    let v = t.f("fwrite", vec![stream2.clone(), s("ABCD")]);
    t.dump(&v);
    t.echo("Stream Contents\n");
    t.f("fseek", vec![stream2.clone(), i(0)]);
    let v = t.f("stream_get_contents", vec![stream2.clone()]);
    t.dump(&v);
    t.echo("Expanding blob size\n");
    let v = t.f("fwrite", vec![stream2.clone(), s("ABCD ABCD ABCD")]);
    t.dump(&v);
    t.echo("Closing Stream\n");
    let v = t.f("fclose", vec![stream2.clone()]);
    t.dump(&v);
    t.echo("Closing database\n");
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    let id = |v: &Value| v.as_resource().unwrap().id();
    assert_eq!(
        t.out(),
        format!(
            "Creating Table\nbool(true)\nPREPARING insert\nBINDING Parameter\nbool(true)\nbool(true)\n\
             Closing statement\nbool(true)\nresource({}) of type (stream)\nStream Contents\nstring(9) \"TEST TEST\"\n\
             Writing to read-only stream\n{}bool(false)\nClosing Stream\nbool(true)\n\
             Opening stream in write mode\nresource({}) of type (stream)\nWriting to blob\nint(4)\n\
             Stream Contents\nstring(9) \"ABCD TEST\"\nExpanding blob size\n{}bool(false)\n\
             Closing Stream\nbool(true)\nClosing database\nbool(true)\n",
            id(&stream),
            warning("fwrite", "Can't write to blob stream: is open as read only"),
            id(&stream2),
            warning(
                "fwrite",
                "It is not possible to increase the size of a BLOB"
            ),
        )
    );

    // A missing row.
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (id INTEGER PRIMARY KEY, data BLOB)");
    let v = t.m(&db, "openBlob", vec![s("test"), s("data"), i(7)]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\n",
            warning("SQLite3::openBlob", "Unable to open blob: no such rowid: 7")
        )
    );
    let r = t.try_m(
        &db,
        "openBlob",
        vec![s("test"), s("data"), i(7), s("ma\0in")],
    );
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "ValueError: SQLite3::openBlob(): Argument #4 ($database) must not contain any null bytes\n"
    );
}

#[test]
fn blob_stream_seeking() {
    // gh20964.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (id INTEGER PRIMARY KEY, data BLOB)");
    t.exec(&db, "INSERT INTO test (id, data) VALUES (1, 'hello')");
    let stream = t.m(&db, "openBlob", vec![s("test"), s("data"), i(1)]);
    let v = t.f("fseek", vec![stream.clone(), i(i64::MIN), i(2)]);
    t.dump(&v);
    t.f("rewind", vec![stream.clone()]);
    let v = t.f("fseek", vec![stream.clone(), i(i64::MIN), i(1)]);
    t.dump(&v);
    assert_eq!(t.out(), "int(-1)\nint(-1)\n");
    // Regular seeks and fstat().
    assert_v!(t.f("fseek", vec![stream.clone(), i(-2), i(2)]), i(0));
    assert_v!(t.f("fread", vec![stream.clone(), i(10)]), s("lo"));
    assert_v!(t.f("fseek", vec![stream.clone(), i(1)]), i(0));
    assert_v!(t.f("fread", vec![stream.clone(), i(3)]), s("ell"));
    assert_v!(t.f("fseek", vec![stream.clone(), i(6)]), i(-1));
    let st = t.f("fstat", vec![stream.clone()]);
    assert_v!(
        st.as_array().unwrap().get_str(b"size").unwrap().clone(),
        i(5)
    );
    t.f("fclose", vec![stream]);
}

#[test]
fn blob_stream_outlives_database() {
    // sqlite3_close_blob_stream.phpt
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (data BLOB)");
    t.exec(&db, "INSERT INTO test (data) VALUES (x'34323432')");
    let stream = t.m(&db, "openBlob", vec![s("test"), s("data"), i(1)]);
    let v = t.m(&db, "close", vec![]);
    t.dump(&v);
    drop(db);
    let v = t.f("fread", vec![stream.clone(), i(4)]);
    t.dump(&v);
    t.f("fclose", vec![stream]);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\nstring(4) \"4242\"\n",
            warning(
                "SQLite3::close",
                "Unable to close database: unable to close due to unfinalized statements or unfinished backups"
            )
        )
    );
}

#[test]
fn bind_streams_as_blobs() {
    // sqlite3_09_blob_bound_param.phpt (a memory stream instead of the
    // userland wrapper of stream_test.inc)
    let mut t = T::new();
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (id STRING, data BLOB)");
    let insert = t.mo(
        &db,
        "prepare",
        vec![s("INSERT INTO test (id, data) VALUES (?, ?)")],
    );
    let res = crate::standard::streams::open_memory(t.vm(), b"abcdefg\0hi");
    let foo = var(Value::Resource(res));
    t.m(&insert, "bindValue", vec![i(1), s("a"), i(3)]);
    let v = t.m(&insert, "bindParam", vec![i(2), Value::Ref(foo), i(4)]);
    t.dump(&v);
    t.m(&insert, "execute", vec![]);
    t.m(&insert, "close", vec![]);
    let results = t.mo(
        &db,
        "query",
        vec![s(
            "SELECT id, quote(data) AS data FROM test ORDER BY id ASC",
        )],
    );
    t.dump_rows(&results, 2);
    assert_eq!(
        t.out(),
        "bool(true)\narray(2) {\n  [0]=>\n  string(1) \"a\"\n  [1]=>\n  string(23) \"X'61626364656667006869'\"\n}\n"
    );

    // sqlite3_blob_bind_resource.phpt: an empty stream binds NULL.
    let insert = t.mo(
        &db,
        "prepare",
        vec![s("INSERT INTO test (id, data) VALUES (1, ?)")],
    );
    let empty = crate::standard::streams::open_memory(t.vm(), b"");
    let f = Value::Resource(empty);
    let v = t.m(
        &insert,
        "bindParam",
        vec![i(1), Value::Ref(var(f.clone())), i(4)],
    );
    t.dump(&v);
    let r = t.m(&insert, "execute", vec![]);
    assert!(r.as_object().is_some());
    t.m(&insert, "close", vec![]);
    t.f("fclose", vec![f.clone()]);
    assert_v!(
        t.m(
            &db,
            "querySingle",
            vec![s("SELECT typeof(data) FROM test WHERE id = 1")]
        ),
        s("null")
    );
    // A closed stream cannot be read.
    let insert = t.mo(&db, "prepare", vec![s("SELECT ?")]);
    t.m(&insert, "bindValue", vec![i(1), f, i(4)]);
    let v = t.m(&insert, "execute", vec![]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "bool(true)\n{}bool(false)\n",
            warning(
                "SQLite3Stmt::execute",
                "Unable to read stream for parameter 1"
            )
        )
    );
}

// ---------------------------------------------------------------------------
// Backup and extensions
// ---------------------------------------------------------------------------

#[test]
fn backup() {
    // sqlite3_38_backup.phpt
    let mut t = T::new();
    let db = t.db();
    t.echo("Creating table\n");
    t.exec(&db, "CREATE TABLE test (a, b);");
    t.exec(&db, "INSERT INTO test VALUES (42, 'php');");
    t.echo("Checking if table has been created\n");
    let v = t.m(
        &db,
        "querySingle",
        vec![s("SELECT COUNT(*) FROM sqlite_master;")],
    );
    t.dump(&v);
    let db2 = t.db();
    t.echo("Backup to DB2\n");
    let v = t.m(&db, "backup", vec![Value::Object(db2.clone())]);
    t.dump(&v);
    t.echo("Checking if table has been copied\n");
    let v = t.m(
        &db2,
        "querySingle",
        vec![s("SELECT COUNT(*) FROM sqlite_master;")],
    );
    t.dump(&v);
    t.echo("Checking backup contents\n");
    let v = t.m(&db2, "querySingle", vec![s("SELECT a FROM test;")]);
    t.dump(&v);
    let v = t.m(&db2, "querySingle", vec![s("SELECT b FROM test;")]);
    t.dump(&v);
    t.echo("Resetting DB2\n");
    t.m(&db2, "close", vec![]);
    let db2 = t.db();
    t.echo("Locking DB1\n");
    let v = t.exec(&db, "BEGIN EXCLUSIVE;");
    t.dump(&v);
    t.echo("Backup to DB2 (should fail)\n");
    let v = t.m(&db, "backup", vec![Value::Object(db2.clone())]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "Creating table\nChecking if table has been created\nint(1)\nBackup to DB2\nbool(true)\n\
             Checking if table has been copied\nint(1)\nChecking backup contents\nint(42)\nstring(3) \"php\"\n\
             Resetting DB2\nLocking DB1\nbool(true)\nBackup to DB2 (should fail)\n{}bool(false)\n",
            warning("SQLite3::backup", "Backup failed: source database is busy")
        )
    );
    // Database names must not contain NUL bytes; the destination must be open.
    let r = t.try_m(&db, "backup", vec![Value::Object(db2.clone()), s("ma\0in")]);
    t.echo_exception(r);
    t.m(&db2, "close", vec![]);
    let r = t.try_m(&db, "backup", vec![Value::Object(db2)]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "ValueError: SQLite3::backup(): Argument #2 ($sourceDatabase) must not contain any null bytes\n\
         Error: The SQLite3 object has not been correctly initialised or is already closed\n"
    );
}

#[test]
fn load_extension() {
    // sqlite3_34_load_extension_ext_dir.phpt
    let mut t = T::new();
    let db = t.db();
    let v = t.m(&db, "loadExtension", vec![s("")]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\n",
            warning("SQLite3::loadExtension", "SQLite Extensions are disabled")
        )
    );

    // sqlite3_33_load_extension_param.phpt
    let dir = std::env::temp_dir();
    t.vm()
        .ini_set_system("sqlite3.extension_dir", dir.to_str().unwrap());
    let r = t.try_m(&db, "loadExtension", vec![s("")]);
    t.echo_exception(r);
    assert_eq!(
        t.out(),
        "ValueError: SQLite3::loadExtension(): Argument #1 ($name) must not be empty\n"
    );

    // sqlite3_22_loadextension.phpt
    let mut t = T::new();
    t.vm().ini_set_system("sqlite3.extension_dir", ".");
    let db = t.db();
    let v = t.m(
        &db,
        "loadExtension",
        vec![s("myext-that-does-not-exist.txt")],
    );
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\n",
            warning(
                "SQLite3::loadExtension",
                "Unable to load extension at './myext-that-does-not-exist.txt'"
            )
        )
    );
    // An existing file outside the configured directory spelling.
    let cwd_file = std::env::current_dir().unwrap().join("Cargo.toml");
    assert!(cwd_file.exists());
    let v = t.m(&db, "loadExtension", vec![s("Cargo.toml")]);
    t.dump(&v);
    assert_eq!(
        t.out(),
        format!(
            "{}bool(false)\n",
            warning(
                "SQLite3::loadExtension",
                "Unable to open extensions outside the defined directory"
            )
        )
    );
    // sqlite3.extension_dir is PHP_INI_SYSTEM.
    assert!(t.vm().ini_set("sqlite3.extension_dir", "/tmp").is_none());
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[test]
fn classes_constants_and_ini() {
    let mut t = T::new();
    for (name, value) in [
        ("SQLITE3_ASSOC", 1),
        ("SQLITE3_NUM", 2),
        ("SQLITE3_BOTH", 3),
        ("SQLITE3_INTEGER", 1),
        ("SQLITE3_FLOAT", 2),
        ("SQLITE3_TEXT", 3),
        ("SQLITE3_BLOB", 4),
        ("SQLITE3_NULL", 5),
        ("SQLITE3_OPEN_READONLY", 1),
        ("SQLITE3_OPEN_READWRITE", 2),
        ("SQLITE3_OPEN_CREATE", 4),
        ("SQLITE3_DETERMINISTIC", 2048),
    ] {
        assert_v!(t.vm().constant(name).unwrap(), i(value), "{name}");
    }
    for (name, value) in [
        ("OK", 0),
        ("DENY", 1),
        ("IGNORE", 2),
        ("CREATE_INDEX", 1),
        ("SELECT", 21),
        ("ATTACH", 24),
        ("FUNCTION", 31),
        ("SAVEPOINT", 32),
        ("COPY", 0),
        ("RECURSIVE", 33),
    ] {
        let v = t.vm().class_constant("SQLite3", name).unwrap().unwrap();
        assert_v!(v, i(value), "SQLite3::{name}");
    }
    let v = t
        .vm()
        .class_constant("SQLite3Stmt", "EXPLAIN_MODE_EXPLAIN_QUERY_PLAN")
        .unwrap()
        .unwrap();
    assert_v!(v, i(2));
    assert_eq!(
        t.vm().ini_get("sqlite3.defensive").map(|v| v.to_string()),
        Some("1".to_string())
    );
    assert_eq!(
        t.vm()
            .ini_get("sqlite3.extension_dir")
            .map(|v| v.to_string()),
        Some(String::new())
    );

    let exc = t.vm().find_class("SQLite3Exception").unwrap();
    assert!(exc.instance_of_name(b"Exception"));
    for class in ["SQLite3", "SQLite3Stmt", "SQLite3Result"] {
        let ce = t.vm().find_class(class).unwrap();
        assert!(ce.has_flag(class_flags::NOT_SERIALIZABLE), "{class}");
        let clone = ce.handlers.clone_obj.expect("clone handler");
        let obj = t.vm().instantiate(&ce).unwrap();
        let r = clone(t.vm(), &obj).map(Value::Object);
        assert_eq!(
            thrown(r).1,
            format!("Trying to clone an uncloneable object of class {class}")
        );
    }
    let stmt = t.vm().find_class("SQLite3Stmt").unwrap();
    assert!(
        stmt.find_method(b"__construct")
            .unwrap()
            .has_flag(member_flags::PRIVATE)
    );
    // Every method signature parses.
    for class in ["SQLite3", "SQLite3Stmt", "SQLite3Result"] {
        let ce = t.vm().find_class(class).unwrap();
        for m in ce.methods.values() {
            let _ = m.signature();
        }
    }

    // sqlite3.defensive=0 lets the schema be written.
    t.vm().ini_set("sqlite3.defensive", "0");
    let db = t.db();
    t.exec(&db, "CREATE TABLE test (a, b);");
    t.exec(&db, "PRAGMA writable_schema = ON;");
    assert_v!(
        t.m(&db, "querySingle", vec![s("DELETE FROM sqlite_master;")]),
        Value::Null
    );
}
