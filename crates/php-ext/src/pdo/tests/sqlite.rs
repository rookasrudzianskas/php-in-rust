//! ext/pdo_sqlite tests: driver attributes, errors, user functions,
//! aggregates, collations, `Pdo\Sqlite`.

use std::rc::Rc;

use php_runtime::class::Function;
use php_runtime::{Args, NativeFunction, PResult, PhpRef, Value, Vm};

use super::harness::*;
use super::phpt;
use crate::pdo::consts::*;

/// `function(&$a, $b) { $a .= $b; return $a; }`
fn agg_concat_step(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let r = args.by_ref(0);
    let mut cur = vm.to_php_string(&r.get())?.to_vec();
    cur.extend_from_slice(&vm.to_php_string(args.get(1))?);
    r.set(Value::from(cur));
    Ok(r.get())
}

/// `function(&$v) { return $v; }`
fn agg_identity(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(args.by_ref(0).get())
}

/// `function() { throw new Exception("boom"); }`
fn throw_boom(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "boom"))
}

/// An authorizer allowing everything but reads of the `secret` table.
fn deny_secret(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let action = args.int(0);
    let table = args.get(1).clone();
    if action == 20 && table.as_str().is_some_and(|t| t.as_bytes() == b"secret") {
        return Ok(Value::Int(1));
    }
    Ok(Value::Int(0))
}

static AGG_STEP: NativeFunction = NativeFunction {
    sig: "test_agg_step(mixed &$a, mixed $b, mixed $v = null): mixed",
    handler: agg_concat_step,
};
static AGG_FINAL: NativeFunction = NativeFunction {
    sig: "test_agg_final(mixed &$v, mixed $rows = null): mixed",
    handler: agg_identity,
};
static THROW_BOOM: NativeFunction = NativeFunction {
    sig: "test_throw_boom(mixed ...$args): mixed",
    handler: throw_boom,
};
static DENY_SECRET: NativeFunction = NativeFunction {
    sig: "test_deny_secret(int $action, ?string $a = null, ?string $b = null, ?string $db = null, ?string $trigger = null): int",
    handler: deny_secret,
};

fn register_test_functions(p: &mut P) {
    for nf in [&AGG_STEP, &AGG_FINAL, &THROW_BOOM, &DENY_SECRET] {
        p.vm().register_function(Rc::new(Function::native(nf)));
    }
}

/// `foreach ($stmt as $row) var_dump($row);`
fn foreach_dump(p: &mut P, stmt: &php_runtime::ObjRef) {
    let it = p.obj(stmt, "getIterator", vec![]);
    while matches!(p.m(&it, "valid", vec![]), Value::Bool(true)) {
        let v = p.m(&it, "current", vec![]);
        p.dump(&v);
        p.m(&it, "next", vec![]);
    }
}

#[test]
fn lastinsertid() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.m(
        &db,
        "query",
        vec![s(
            "CREATE TABLE test_pdo_sqlite_lastinsertid (id INT AUTO INCREMENT, name TEXT)",
        )],
    );
    p.m(
        &db,
        "query",
        vec![s(
            "INSERT INTO test_pdo_sqlite_lastinsertid VALUES (NULL, 'PHP'), (NULL, 'PHP6')",
        )],
    );
    let r = p.m(
        &db,
        "query",
        vec![s("SELECT * FROM test_pdo_sqlite_lastinsertid")],
    );
    p.dump(&r);
    let r = p.m(&db, "errorInfo", vec![]);
    p.dump(&r);
    let r = p.m(&db, "lastInsertId", vec![]);
    p.dump(&r);
    let out = p.output();
    let (_, want) = expect_section(phpt!("pdo_sqlite/tests/pdo_sqlite_lastinsertid.phpt"));
    // Object handles depend on the objects created before the script.
    let want = want.replace("#2 (1)", "#%d (1)");
    assert!(
        expectf_match(want.trim().as_bytes(), out.trim().as_bytes()),
        "{out}"
    );
}

#[test]
fn transaction_locked_table_warning() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(1)]);
    p.m(&db, "beginTransaction", vec![]);
    p.m(
        &db,
        "query",
        vec![s(
            "CREATE TABLE test_pdo_sqlite_transaction (id INT AUTO INCREMENT, name TEXT)",
        )],
    );
    p.m(&db, "commit", vec![]);
    p.m(&db, "beginTransaction", vec![]);
    p.m(
        &db,
        "query",
        vec![s(
            "INSERT INTO test_pdo_sqlite_transaction VALUES (NULL, 'PHP'), (NULL, 'PHP6')",
        )],
    );
    p.m(&db, "rollback", vec![]);
    let r = p.obj(
        &db,
        "query",
        vec![s("SELECT COUNT(*) FROM test_pdo_sqlite_transaction")],
    );
    let n = p.m(&r, "rowCount", vec![]);
    p.dump(&n);
    p.m(
        &db,
        "query",
        vec![s("DROP TABLE test_pdo_sqlite_transaction")],
    );
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_transaction.phpt"));
}

#[test]
fn extended_result_codes_attribute() {
    let mut p = P::new();
    let run = |p: &mut P, options: Option<Value>, label: &str| {
        let args = match options {
            Some(o) => vec![s("sqlite::memory:"), s(""), s(""), o],
            None => vec![s("sqlite::memory:")],
        };
        let db = p.new_obj("PDO", args);
        p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(0)]);
        p.exec(
            &db,
            "CREATE TABLE dog ( id INTEGER PRIMARY KEY, name TEXT, annoying INTEGER )",
        );
        p.echo("Inserting first time which should succeed\n");
        p.exec(&db, "INSERT INTO dog VALUES (1, 'Annoying Dog', 1)");
        let info = p.m(&db, "errorInfo", vec![]);
        let info = info.as_array().cloned().unwrap();
        let code = |v: Option<&Value>| match v {
            Some(Value::String(s)) => s.to_string_lossy().into_owned(),
            Some(Value::Int(n)) => n.to_string(),
            _ => String::new(),
        };
        p.echo(format!(
            "First{label} Error Info: SQLSTATE Error Code: ({}), Driver Specific Error Code: ({})\n",
            code(info.get_int(0)),
            code(info.get_int(1))
        ));
        p.echo("Inserting second time which should fail\n");
        p.exec(&db, "INSERT INTO dog VALUES (1, 'Annoying Dog', 1)");
        let info = p.m(&db, "errorInfo", vec![]);
        let info = info.as_array().cloned().unwrap();
        p.echo(format!(
            "Second{label} Error Info: SQLSTATE Error Code: ({}), Driver Specific Error Code: ({})\n",
            code(info.get_int(0)),
            code(info.get_int(1))
        ));
    };
    p.echo("Creating new PDO\n");
    run(&mut p, None, "");
    p.echo("Creating new PDO with Extended Result Codes turned on\n");
    let opts = iassoc(vec![(1002, Value::Bool(true))]);
    run(&mut p, Some(opts), " (Extended)");
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_extendederror_attr.phpt"));
}

#[test]
fn empty_filename() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite:")]);
    let r = p.exec(&db, "CREATE TABLE test_sqlite_empty_filename (id INT);");
    p.dump(&r);
    let db = p.new_obj("PDO", vec![s("sqlite:file:?cache=shared")]);
    let r = p.exec(&db, "CREATE TABLE test_sqlite_empty_filename (id INT);");
    p.dump(&r);
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_empty_filename.phpt"));
}

#[test]
fn get_attribute_versions() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    for attr in [ATTR_SERVER_VERSION, ATTR_CLIENT_VERSION] {
        let r = p.m(&db, "getAttribute", vec![i(attr)]);
        p.dump(&r);
    }
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_get_attribute.phpt"));
}

#[test]
fn statement_readonly_attribute() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let st = p.obj(&db, "prepare", vec![s("SELECT 1;")]);
    let r = p.m(&st, "getAttribute", vec![i(1001)]);
    p.dump(&r);
    let st = p.obj(
        &db,
        "prepare",
        vec![s("CREATE TABLE test_sqlite_stmt_getattribute (a TEXT);")],
    );
    let r = p.m(&st, "getAttribute", vec![i(1001)]);
    p.dump(&r);
    p.check(phpt!(
        "pdo_sqlite/tests/pdo_sqlite_statement_getattribute.phpt"
    ));
}

#[test]
fn create_function() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.m(
        &db,
        "query",
        vec![s(
            "CREATE TABLE test_pdo_sqlite_createfunction (id INT AUTO INCREMENT, name TEXT)",
        )],
    );
    p.m(
        &db,
        "query",
        vec![s(
            "INSERT INTO test_pdo_sqlite_createfunction VALUES (NULL, 'PHP'), (NULL, 'PHP6')",
        )],
    );
    p.m(
        &db,
        "sqliteCreateFunction",
        vec![s("testing"), s("strtolower")],
    );
    let stmt = p.obj(
        &db,
        "query",
        vec![s(
            "SELECT testing(name) FROM test_pdo_sqlite_createfunction",
        )],
    );
    foreach_dump(&mut p, &stmt);
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_createfunction.phpt"));
}

#[test]
fn create_function_with_flags() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.m(&db, "query", vec![s("CREATE TABLE test_pdo_sqlite_createfunction_with_flags (id INT AUTO INCREMENT, name TEXT)")]);
    p.m(&db, "query", vec![s("INSERT INTO test_pdo_sqlite_createfunction_with_flags VALUES (NULL, 'PHP'), (NULL, 'PHP6')")]);
    p.m(
        &db,
        "sqliteCreateFunction",
        vec![s("testing"), s("strtolower"), i(1), i(2048)],
    );
    let stmt = p.obj(
        &db,
        "query",
        vec![s(
            "SELECT testing(name) FROM test_pdo_sqlite_createfunction_with_flags",
        )],
    );
    foreach_dump(&mut p, &stmt);
    p.check(phpt!(
        "pdo_sqlite/tests/pdo_sqlite_createfunction_with_flags.phpt"
    ));
}

#[test]
fn create_aggregate() {
    let mut p = P::new();
    register_test_functions(&mut p);
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.m(
        &db,
        "query",
        vec![s(
            "CREATE TABLE test_pdo_sqlite_createaggregate (id INT AUTO INCREMENT, name TEXT)",
        )],
    );
    p.m(
        &db,
        "query",
        vec![s(
            "INSERT INTO test_pdo_sqlite_createaggregate VALUES (NULL, 'PHP'), (NULL, 'PHP6')",
        )],
    );
    p.m(
        &db,
        "sqliteCreateAggregate",
        vec![s("testing"), s("test_agg_step"), s("test_agg_final")],
    );
    let stmt = p.obj(
        &db,
        "query",
        vec![s(
            "SELECT testing(name) FROM test_pdo_sqlite_createaggregate",
        )],
    );
    foreach_dump(&mut p, &stmt);
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_createaggregate.phpt"));
}

#[test]
fn create_collation() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(2)]);
    p.m(
        &db,
        "query",
        vec![s(
            "CREATE TABLE test_pdo_sqlite_createcollation (id INT AUTO INCREMENT, name TEXT)",
        )],
    );
    p.m(&db, "query", vec![s("INSERT INTO test_pdo_sqlite_createcollation VALUES (NULL, '1'), (NULL, '2'), (NULL, '10')")]);
    p.m(
        &db,
        "sqliteCreateCollation",
        vec![s("MYCOLLATE"), s("strnatcmp")],
    );
    for sql in [
        "SELECT name FROM test_pdo_sqlite_createcollation ORDER BY name COLLATE MYCOLLATE",
        "SELECT name FROM test_pdo_sqlite_createcollation ORDER BY name",
    ] {
        let result = p.obj(&db, "query", vec![s(sql)]);
        let it = p.obj(&result, "getIterator", vec![]);
        while matches!(p.m(&it, "valid", vec![]), Value::Bool(true)) {
            let row = p.m(&it, "current", vec![]);
            let name = row.as_array().unwrap().get_str(b"name").cloned().unwrap();
            p.echo(name.as_str().unwrap().as_bytes());
            p.echo("\n");
            p.m(&it, "next", vec![]);
        }
    }
    // `function($a, $b) { return $a; }`: trim($a, $b) returns a string too.
    p.m(
        &db,
        "sqliteCreateCollation",
        vec![s("MYCOLLATEBAD"), s("trim")],
    );
    let r = p.tm(
        &db,
        "query",
        vec![s(
            "SELECT name FROM test_pdo_sqlite_createcollation ORDER BY name COLLATE MYCOLLATEBAD",
        )],
    );
    p.catch(r);
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_createcollation.phpt"));
}

#[test]
fn failed_execute_invalidates_previous_result() {
    let mut p = P::new();
    register_test_functions(&mut p);
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s("SELECT ? AS value UNION ALL SELECT 2")],
    );
    p.m(&stmt, "execute", vec![arr(vec![s("first")])]);
    let r = p.m(&stmt, "fetchColumn", vec![]);
    p.dump(&r);
    let std = p.new_obj("stdClass", vec![]);
    let r = p.tm(&stmt, "execute", vec![arr(vec![Value::Object(std)])]);
    p.catch(r);
    let r = p.m(&stmt, "fetchColumn", vec![]);
    p.dump(&r);
    p.m(&stmt, "execute", vec![arr(vec![s("third")])]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_COLUMN)]);
    p.dump(&r);
    let out = p.output();
    assert_eq!(
        out,
        "string(5) \"first\"\nError: Object of class stdClass could not be converted to string\nbool(false)\narray(2) {\n  [0]=>\n  string(5) \"third\"\n  [1]=>\n  int(2)\n}\n"
    );

    // A user function that throws: the exception propagates and the
    // statement's previous result is gone.
    p.m(
        &db,
        "sqliteCreateFunction",
        vec![s("fail_execute"), s("test_throw_boom")],
    );
    p.output();
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s("SELECT fail_execute(?) AS value UNION ALL SELECT 2")],
    );
    let r = p.tm(&stmt, "execute", vec![arr(vec![Value::Bool(true)])]);
    p.catch(r);
    let r = p.m(&stmt, "fetchColumn", vec![]);
    p.dump(&r);
    assert_eq!(p.output(), "Exception: boom\nbool(false)\n");

    // ERRMODE_WARNING: a constraint violation warns once.
    p.exec(&db, "CREATE TABLE failed_execute (value INTEGER UNIQUE)");
    p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(1)]);
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s("INSERT INTO failed_execute VALUES (?)")],
    );
    p.m(&stmt, "execute", vec![arr(vec![i(1)])]);
    let r = p.m(&stmt, "execute", vec![arr(vec![i(1)])]);
    p.dump(&r);
    let r = p.m(&stmt, "fetchColumn", vec![]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "\nWarning: PDOStatement::execute(): SQLSTATE[23000]: Integrity constraint violation: 19 UNIQUE constraint failed: failed_execute.value in /t.php on line 3\nbool(false)\nbool(false)\n"
    );
}

#[test]
fn tostring_exception_during_binding() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.exec(&db, "CREATE TABLE t(id int, v varchar(255))");
    let stmt = p.obj(&db, "prepare", vec![s("INSERT INTO t VALUES(:i, :v)")]);
    p.m(&stmt, "bindValue", vec![s("i"), i(1234)]);
    let param2 = PhpRef::new(s("foo"));
    p.m(&stmt, "bindParam", vec![s("v"), Value::Ref(param2.clone())]);
    // An object without __toString() fails the conversion like a throwing one.
    let o = p.new_obj("stdClass", vec![]);
    param2.set(Value::Object(o));
    for _ in 0..2 {
        if p.tm(&stmt, "execute", vec![]).is_err() {
            p.echo("Exception thrown ...\n");
        }
    }
    let q = p.obj(&db, "query", vec![s("SELECT * FROM t")]);
    let r = p.m(&q, "fetch", vec![i(FETCH_ASSOC)]);
    assert!(matches!(r, Value::Bool(false)));
    p.check(phpt!("pdo_sqlite/tests/pdo_sqlite_tostring_exception.phpt"));
}

#[test]
fn deprecated_driver_methods() {
    let mut p = P::new();
    register_test_functions(&mut p);
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.m(
        &db,
        "sqliteCreateFunction",
        vec![s("test1"), s("strtolower")],
    );
    p.m(
        &db,
        "sqliteCreateAggregate",
        vec![s("test2"), s("test_agg_step"), s("test_agg_final")],
    );
    p.m(
        &db,
        "sqliteCreateCollation",
        vec![s("test3"), s("strnatcmp")],
    );
    let out = p.output();
    assert_eq!(
        out,
        "\nDeprecated: Method PDO::sqliteCreateFunction() is deprecated since 8.5, use Pdo\\Sqlite::createFunction() instead in /t.php on line 3\n\
         \nDeprecated: Method PDO::sqliteCreateAggregate() is deprecated since 8.5, use Pdo\\Sqlite::createAggregate() instead in /t.php on line 3\n\
         \nDeprecated: Method PDO::sqliteCreateCollation() is deprecated since 8.5, use Pdo\\Sqlite::createCollation() instead in /t.php on line 3\n"
    );
    // Values of the deprecated aliases.
    for (name, v) in [
        ("SQLITE_ATTR_EXTENDED_RESULT_CODES", 1002),
        ("SQLITE_ATTR_OPEN_FLAGS", 1000),
        ("SQLITE_ATTR_READONLY_STATEMENT", 1001),
        ("SQLITE_DETERMINISTIC", 2048),
        ("SQLITE_OPEN_READONLY", 1),
        ("SQLITE_OPEN_READWRITE", 2),
        ("SQLITE_OPEN_CREATE", 4),
    ] {
        assert_eq!(
            p.vm()
                .class_constant("PDO", name)
                .unwrap()
                .and_then(|c| c.as_int()),
            Some(v),
            "{name}"
        );
    }
}

#[test]
fn pdo_sqlite_subclass_and_connect() {
    let mut p = P::new();
    register_test_functions(&mut p);
    let db = p.ts("PDO", "connect", vec![s("sqlite::memory:")]).unwrap();
    let db = db.as_object().cloned().unwrap();
    assert_eq!(db.class().name.as_bytes(), b"Pdo\\Sqlite");
    p.m(&db, "createFunction", vec![s("up"), s("strtoupper")]);
    let st = p.obj(&db, "query", vec![s("SELECT up('abc')")]);
    let r = p.m(&st, "fetchColumn", vec![]);
    p.dump(&r);
    let r = p
        .ts("Pdo\\Sqlite", "connect", vec![s("sqlite::memory:")])
        .unwrap();
    p.echo(format!("{}\n", r.as_object().unwrap().class().name));
    let sq = p.new_obj("Pdo\\Sqlite", vec![s("sqlite::memory:")]);
    p.m(
        &sq,
        "exec",
        vec![s("CREATE TABLE secret(x); CREATE TABLE open(y)")],
    );
    p.m(&sq, "setAuthorizer", vec![s("test_deny_secret")]);
    let r = p.tm(&sq, "query", vec![s("SELECT * FROM secret")]);
    p.catch(r);
    let st = p.obj(&sq, "query", vec![s("SELECT * FROM open")]);
    let r = p.m(&st, "fetchAll", vec![]);
    p.dump(&r);
    p.m(&sq, "setAuthorizer", vec![Value::Null]);
    let st = p.obj(&sq, "query", vec![s("SELECT count(*) FROM secret")]);
    let r = p.m(&st, "fetchColumn", vec![]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "string(3) \"ABC\"\nPdo\\Sqlite\nPDOException: SQLSTATE[HY000]: General error: 23 access to secret.x is prohibited\narray(0) {\n}\nint(0)\n"
    );
}

#[test]
fn user_function_exception_propagates() {
    let mut p = P::new();
    register_test_functions(&mut p);
    let db = p.new_obj("Pdo\\Sqlite", vec![s("sqlite::memory:")]);
    p.m(&db, "createFunction", vec![s("boom"), s("test_throw_boom")]);
    let r = p.tm(&db, "query", vec![s("SELECT boom()")]);
    p.catch(r);
    let r = p.tm(&db, "exec", vec![s("SELECT boom()")]);
    p.catch(r);
    assert_eq!(p.output(), "Exception: boom\nException: boom\n");
}
