//! Unit tests ported from php-src `ext/sqlite3/tests/*.phpt`.
//!
//! Each test replays the phpt script step by step against an in-memory
//! database and compares the produced output (echoes, `var_dump()`s,
//! warnings) with the phpt expectation. Userland callbacks cannot run yet:
//! callbacks are native functions registered by the tests (or built-ins
//! such as `strtoupper`), with any state they need in a thread-local.

use std::rc::Rc;

use php_runtime::class::Function;
use php_runtime::value::float_fmt;
use php_runtime::{ArrayKey, CallArgs, Flow, NativeFunction, ObjRef, PResult, Value, Vm};

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
