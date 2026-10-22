//! `Pdo\Sqlite` tests (`ext/pdo_sqlite/tests/subclasses`).

use php_runtime::{Args, NativeFunction, PResult, Value, Vm};

use super::harness::*;
use super::phpt;
use crate::pdo::consts::*;

const SQLITE: &str = "Pdo\\Sqlite";

fn testing_table(p: &mut P, db: &php_runtime::ObjRef, table: &str) {
    p.m(
        db,
        "query",
        vec![s(&format!(
            "CREATE TABLE {table} (id INT AUTO INCREMENT, name TEXT)"
        ))],
    );
    p.m(
        db,
        "query",
        vec![s(&format!("INSERT INTO {table} VALUES (NULL, 'PHP')"))],
    );
    p.m(
        db,
        "query",
        vec![s(&format!("INSERT INTO {table} VALUES (NULL, 'PHP6')"))],
    );
    p.m(
        db,
        "createFunction",
        vec![s("testing"), s("strtolower"), i(1), i(2048)],
    );
    let st = p.obj(
        db,
        "query",
        vec![s(&format!("SELECT testing(name) FROM {table}"))],
    );
    let it = p.obj(&st, "getIterator", vec![]);
    while matches!(p.m(&it, "valid", vec![]), Value::Bool(true)) {
        let v = p.m(&it, "current", vec![]);
        p.dump(&v);
        p.m(&it, "next", vec![]);
    }
    p.echo("Fin.");
}

#[test]
fn pdosqlite_001_basic() {
    let mut p = P::new();
    let db = p.new_obj(SQLITE, vec![s("sqlite::memory:")]);
    testing_table(&mut p, &db, "pdosqlite_001");
    p.check(phpt!("pdo_sqlite/tests/subclasses/pdosqlite_001.phpt"));
}

#[test]
fn pdosqlite_002_connect() {
    let mut p = P::new();
    let db = p.ts("PDO", "connect", vec![s("sqlite::memory:")]).unwrap();
    let db = db.as_object().cloned().unwrap();
    assert!(db.class().instance_of_name(SQLITE.as_bytes()));
    testing_table(&mut p, &db, "pdosqlite_002");
    p.check(phpt!("pdo_sqlite/tests/subclasses/pdosqlite_002.phpt"));
}

#[test]
fn pdosqlite_006_connect_through_subclass() {
    let mut p = P::new();
    user_class(p.vm(), "MyPdoSqlite", Some(SQLITE), &[], None);
    let db = p
        .ts("MyPdoSqlite", "connect", vec![s("sqlite::memory:")])
        .unwrap();
    assert_eq!(
        db.as_object().unwrap().class().name.as_bytes(),
        b"MyPdoSqlite"
    );
    // A plain PDO subclass keeps the legacy behaviour with new, but
    // connect() requires the driver class.
    user_class(p.vm(), "MyPdo", Some("PDO"), &[], None);
    let db = p.new_obj("MyPdo", vec![s("sqlite::memory:")]);
    assert_eq!(db.class().name.as_bytes(), b"MyPdo");
    let r = p.ts("MyPdo", "connect", vec![s("sqlite::memory:")]);
    p.catch(r);
    assert_eq!(
        p.output(),
        "PDOException: MyPdo::connect() cannot be used for connecting to the \"sqlite\" driver, either call Pdo\\Sqlite::connect() or PDO::connect() instead\n"
    );
}

#[test]
fn pdosqlite_004_blob_open() {
    let mut p = P::new();
    let db = p.new_obj(SQLITE, vec![s("sqlite::memory:")]);
    p.echo("Creating Table\n");
    p.exec(&db, "CREATE TABLE test (id STRING, data BLOB)");
    p.echo("PREPARING insert\n");
    let ins = p.obj(
        &db,
        "prepare",
        vec![s("INSERT INTO test (id, data) VALUES (?, ?)")],
    );
    p.echo("BINDING Parameter\n");
    let r = p.m(&ins, "bindValue", vec![i(1), s("a"), i(PARAM_STR)]);
    p.dump(&r);
    let r = p.m(&ins, "bindValue", vec![i(2), s("TEST TEST"), i(PARAM_LOB)]);
    p.dump(&r);
    p.m(&ins, "execute", vec![]);
    p.echo("\n");
    let stream = p.m(&db, "openBlob", vec![s("test"), s("data"), i(1)]);
    p.dump(&stream);
    p.echo("Stream Contents\n");
    let r =
        p.t.call("stream_get_contents", vec![stream.clone()])
            .unwrap();
    p.dump(&r);
    p.echo("Writing to read-only stream\n");
    let r = p.t.call("fwrite", vec![stream.clone(), s("ABCD")]).unwrap();
    p.dump(&r);
    p.echo("Closing Stream\n");
    let r = p.t.call("fclose", vec![stream]).unwrap();
    p.dump(&r);
    p.echo("Opening stream in write mode\n");
    let stream = p.m(
        &db,
        "openBlob",
        vec![s("test"), s("data"), i(1), s("main"), i(2)],
    );
    p.dump(&stream);
    p.echo("Writing to blob\n");
    let r = p.t.call("fwrite", vec![stream.clone(), s("ABCD")]).unwrap();
    p.dump(&r);
    p.echo("Stream Contents\n");
    p.t.call("fseek", vec![stream.clone(), i(0)]).unwrap();
    let r =
        p.t.call("stream_get_contents", vec![stream.clone()])
            .unwrap();
    p.dump(&r);
    p.echo("Expanding blob size\n");
    let r =
        p.t.call("fwrite", vec![stream.clone(), s("ABCD ABCD ABCD")])
            .unwrap();
    p.dump(&r);
    p.echo("Closing Stream\n");
    let r = p.t.call("fclose", vec![stream]).unwrap();
    p.dump(&r);
    p.echo("Done\n");
    p.check(phpt!(
        "pdo_sqlite/tests/subclasses/pdosqlite_004_blobopen.phpt"
    ));
}

#[test]
fn open_blob_failure_warns() {
    let mut p = P::new();
    let db = p.new_obj(SQLITE, vec![s("sqlite::memory:")]);
    let r = p.m(&db, "openBlob", vec![s("nope"), s("data"), i(1)]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "\nWarning: Unable to open blob: no such table: main.nope in /t.php on line 3\nbool(false)\n"
    );
}

#[test]
fn getattr_busy() {
    let mut p = P::new();
    let db = p.new_obj(SQLITE, vec![s("sqlite::memory:")]);
    p.m(&db, "query", vec![s("CREATE TABLE test_busy (a string);")]);
    p.m(
        &db,
        "query",
        vec![s(
            "INSERT INTO test_busy VALUES ('interleaved'), ('statements')",
        )],
    );
    let st = p.obj(&db, "prepare", vec![s("SELECT a FROM test_busy")]);
    let r = p.m(&st, "getAttribute", vec![i(1003)]);
    p.dump(&r);
    p.m(&st, "execute", vec![]);
    let r = p.m(&st, "getAttribute", vec![i(1003)]);
    p.dump(&r);
    p.check(phpt!(
        "pdo_sqlite/tests/subclasses/pdo_sqlite_getattr_busy.phpt"
    ));
}

#[test]
fn getsetattr_explain() {
    let mut p = P::new();
    let db = p.new_obj(SQLITE, vec![s("sqlite::memory:")]);
    p.m(
        &db,
        "query",
        vec![s("CREATE TABLE test_explain (a string);")],
    );
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s(
            "INSERT INTO test_explain VALUES ('first insert'), ('second_insert')",
        )],
    );
    p.m(&stmt, "setAttribute", vec![i(1004), i(1)]);
    let r = p.m(&stmt, "getAttribute", vec![i(1004)]);
    assert_eq!(r.as_int(), Some(1));
    p.m(&stmt, "execute", vec![]);
    let rows = p.m(&stmt, "fetchAll", vec![i(FETCH_ASSOC)]);
    let rows = rows.as_array().cloned().unwrap();
    let first = rows.get_int(0).and_then(|r| r.as_array().cloned()).unwrap();
    let keys: Vec<String> = first
        .keys()
        .map(|k| k.to_php_str().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        keys,
        ["addr", "opcode", "p1", "p2", "p3", "p4", "p5", "comment"]
    );
    assert_eq!(
        first
            .get_str(b"opcode")
            .and_then(|v| v.as_str())
            .map(|s| s.to_vec()),
        Some(b"Init".to_vec())
    );
    // The INSERT ran as EXPLAIN: nothing was inserted.
    let stmts = p.obj(&db, "prepare", vec![s("SELECT * FROM test_explain")]);
    p.m(&stmts, "setAttribute", vec![i(1004), i(2)]);
    p.m(&stmts, "execute", vec![]);
    let plan = p.m(&stmts, "fetchAll", vec![i(FETCH_ASSOC)]);
    assert!(!plan.as_array().unwrap().is_empty());
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s(
            "INSERT INTO test_explain VALUES ('first insert'), ('second_insert')",
        )],
    );
    p.m(&stmt, "setAttribute", vec![i(1004), i(0)]);
    p.m(&stmt, "execute", vec![]);
    p.m(&stmts, "setAttribute", vec![i(1004), i(0)]);
    p.m(&stmts, "execute", vec![]);
    let r = p.m(&stmts, "fetchAll", vec![i(FETCH_ASSOC)]);
    p.dump(&r);
    user_class(p.vm(), "Duh", None, &[], None);
    let duh = p.new_obj("Duh", vec![]);
    for v in [s("EXPLAIN"), Value::Object(duh), i(-1), i(256)] {
        let r = p.tm(&stmts, "setAttribute", vec![i(1004), v]);
        p.catch(r);
    }
    let r = p.m(&stmts, "getAttribute", vec![i(1004)]);
    p.dump(&Value::Bool(r.as_int() == Some(0)));
    assert_eq!(
        p.output(),
        "array(2) {\n  [0]=>\n  array(1) {\n    [\"a\"]=>\n    string(12) \"first insert\"\n  }\n  [1]=>\n  array(1) {\n    [\"a\"]=>\n    string(13) \"second_insert\"\n  }\n}\n\
         TypeError: explain mode must be of type int, string given\n\
         TypeError: explain mode must be of type int, Duh given\n\
         ValueError: explain mode must be one of the Pdo\\Sqlite::EXPLAIN_MODE_* constants\n\
         ValueError: explain mode must be one of the Pdo\\Sqlite::EXPLAIN_MODE_* constants\n\
         bool(true)\n"
    );
}

#[test]
fn transaction_mode() {
    let mut p = P::new();
    let dsn = "sqlite:file:foo?mode=memory&cache=shared";
    let pdo = p
        .ts("PDO", "connect", vec![s(dsn)])
        .unwrap()
        .as_object()
        .cloned()
        .unwrap();
    let pdo2 = p
        .ts("PDO", "connect", vec![s(dsn)])
        .unwrap()
        .as_object()
        .cloned()
        .unwrap();
    let cur = p.m(&pdo, "getAttribute", vec![i(1005)]).as_int();
    p.dump(&Value::Bool(cur == Some(0)));
    for mode in [0, 1, 2, 0, 1, 2] {
        let r = p.m(&pdo, "setAttribute", vec![i(1005), i(mode)]);
        p.dump(&r);
        let cur = p.m(&pdo, "getAttribute", vec![i(1005)]).as_int();
        p.dump(&Value::Bool(cur == Some(mode)));
    }
    let r = p.m(&pdo, "setAttribute", vec![i(1005), i(4)]);
    p.dump(&r);
    let attempt = |p: &mut P, mode: i64, rollback2: bool| {
        p.m(&pdo, "setAttribute", vec![i(1005), i(mode)]);
        p.m(&pdo, "beginTransaction", vec![]);
        match p.tm(&pdo2, "exec", vec![s("begin immediate transaction")]) {
            Ok(_) => {
                if rollback2 {
                    p.m(&pdo2, "rollBack", vec![]);
                }
                p.echo("Database is not locked\n");
            }
            Err(e) => {
                let msg = describe(&e);
                let msg = msg.trim_start_matches("PDOException: ");
                p.echo(format!("Database is locked: {msg}\n"));
            }
        }
    };
    attempt(&mut p, 0, true);
    p.m(&pdo, "rollBack", vec![]);
    attempt(&mut p, 1, false);
    p.m(&pdo, "rollBack", vec![]);
    attempt(&mut p, 2, false);
    p.check(phpt!(
        "pdo_sqlite/tests/subclasses/pdo_sqlite_transaction_mode.phpt"
    ));
}

/// `function (int $action) { return $action == 21 ? OK : DENY; }`
fn allow_select(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(if args.int(0) == 21 { 0 } else { 1 }))
}

const ACTIONS: [&str; 24] = [
    "COPY",
    "CREATE_INDEX",
    "CREATE_TABLE",
    "CREATE_TEMP_INDEX",
    "CREATE_TEMP_TABLE",
    "CREATE_TEMP_TRIGGER",
    "CREATE_TEMP_VIEW",
    "CREATE_TRIGGER",
    "CREATE_VIEW",
    "DELETE",
    "DROP_INDEX",
    "DROP_TABLE",
    "DROP_TEMP_INDEX",
    "DROP_TEMP_TABLE",
    "DROP_TEMP_TRIGGER",
    "DROP_TEMP_VIEW",
    "DROP_TRIGGER",
    "DROP_VIEW",
    "INSERT",
    "PRAGMA",
    "READ",
    "SELECT",
    "TRANSACTION",
    "UPDATE",
];

/// Dumps the action name and the remaining arguments.
fn dump_args(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let action = args.int(0) as usize;
    let mut rest = Vec::new();
    for v in args.rest(1) {
        rest.push(vm.to_php_string(v)?.to_string_lossy().into_owned());
    }
    let mut out = Vec::new();
    for v in [Value::from(ACTIONS[action]), Value::from(rest.join(","))] {
        var_dump_into(vm, &mut out, &v, 0);
    }
    vm.echo(&out)?;
    Ok(Value::Int(0))
}

fn return_fail(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(s("FAIL"))
}

fn return_4200(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(i(4200))
}

static ALLOW_SELECT: NativeFunction = NativeFunction {
    sig: "test_allow_select(int $action, mixed ...$rest): int",
    handler: allow_select,
};
static DUMP_ARGS: NativeFunction = NativeFunction {
    sig: "test_dump_args(int $action, mixed ...$rest): int",
    handler: dump_args,
};
static RETURN_FAIL: NativeFunction = NativeFunction {
    sig: "test_return_fail(mixed ...$rest): string",
    handler: return_fail,
};
static RETURN_4200: NativeFunction = NativeFunction {
    sig: "test_return_4200(mixed ...$rest): int",
    handler: return_4200,
};

#[test]
fn set_authorizer() {
    let mut p = P::new();
    for nf in [&ALLOW_SELECT, &DUMP_ARGS, &RETURN_FAIL, &RETURN_4200] {
        user_function(p.vm(), nf);
    }
    let db = p.new_obj(SQLITE, vec![s("sqlite::memory:")]);
    p.m(&db, "setAuthorizer", vec![s("test_allow_select")]);
    let r = p.m(&db, "query", vec![s("SELECT 1;")]);
    p.dump(&r);
    let r = p.tm(&db, "exec", vec![s("CREATE TABLE test (a, b);")]);
    if let Some(v) = p.catch(r) {
        p.dump(&v);
    }
    p.m(&db, "setAuthorizer", vec![Value::Null]);
    let r = p.m(
        &db,
        "exec",
        vec![s("CREATE TABLE test (a); INSERT INTO test VALUES (42);")],
    );
    p.dump(&r);
    let r = p.m(&db, "exec", vec![s("SELECT a FROM test;")]);
    p.dump(&r);
    p.m(&db, "setAuthorizer", vec![s("test_dump_args")]);
    let r = p.m(&db, "exec", vec![s("SELECT * FROM test WHERE a = 42;")]);
    p.dump(&r);
    let r = p.m(&db, "exec", vec![s("DROP TABLE test;")]);
    p.dump(&r);
    for cb in ["test_return_fail", "test_return_4200"] {
        p.m(&db, "setAuthorizer", vec![s(cb)]);
        let r = p.tm(&db, "query", vec![s("SELECT 1;")]);
        if let Some(v) = p.catch(r) {
            p.dump(&v);
        }
    }
    p.check(phpt!(
        "pdo_sqlite/tests/subclasses/pdosqlite_setauthorizer.phpt"
    ));
}
