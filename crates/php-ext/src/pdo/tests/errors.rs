//! Error modes, exceptions and argument validation.

use php_runtime::class::class_flags;
use php_runtime::{Args, NativeFunction, PResult, PhpRef, Value, Vm};

use super::harness::*;
use super::phpt;
use crate::pdo::consts::*;

#[test]
fn set_fetch_mode_errors() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "CREATE TABLE pdo_set_fetch_mode_error(id int NOT NULL PRIMARY KEY, val VARCHAR(10), val2 VARCHAR(10))");
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s("SELECT id, val, val2 from pdo_set_fetch_mode_error")],
    );
    for flags in [0, FETCH_PROPS_LATE] {
        for mode in FETCH_KEY_PAIR..=16 {
            p.echo(format!("Mode: {mode}\n"));
            let r = p.tm(&stmt, "setFetchMode", vec![i(mode | flags)]);
            if let Some(v) = p.catch(r) {
                p.dump(&v);
            }
        }
    }
    for mode in [
        FETCH_KEY_PAIR | FETCH_PROPS_LATE,
        FETCH_KEY_PAIR | (FETCH_SERIALIZE * 2),
        FETCH_FUNC,
    ] {
        let r = p.tm(&stmt, "setFetchMode", vec![i(mode)]);
        if let Some(v) = p.catch(r) {
            p.dump(&v);
        }
    }
    p.check(phpt!("pdo/tests/pdo_set_fetch_mode_errors.phpt"));
}

#[test]
fn bug_44173_query_fetch_mode_arguments() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "CREATE TABLE test44173 (x int)");
    p.exec(&db, "INSERT INTO test44173 VALUES (1)");
    let q = s("SELECT * FROM test44173");
    let cases: Vec<Vec<Value>> = vec![
        vec![q.clone(), i(FETCH_LAZY), i(0), arr(vec![])],
        vec![q.clone(), s("abc")],
        vec![q.clone(), i(FETCH_CLASS), i(0), i(0), i(0)],
        vec![q.clone(), i(FETCH_INTO)],
        vec![q.clone(), i(FETCH_COLUMN)],
        vec![q.clone(), i(FETCH_CLASS)],
    ];
    for args in cases {
        let r = p.tm(&db, "query", args);
        match r {
            Ok(v) => p.dump(&v),
            Err(php_runtime::Flow::Exception(e)) => {
                let msg = Vm::exception_message(&e);
                p.echo(format!("{msg}\n"));
            }
            Err(other) => panic!("{other:?}"),
        }
    }
    p.check(phpt!("pdo/tests/bug_44173.phpt"));
}

#[test]
fn stmt_class_ctor_errors() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "CREATE TABLE pdo_fetch_all_class_ctor_error(id int NOT NULL PRIMARY KEY, val VARCHAR(10), val2 VARCHAR(10))");
    p.exec(
        &db,
        "INSERT INTO pdo_fetch_all_class_ctor_error VALUES(1, 'A', 'AA')",
    );
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s(
            "SELECT id, val, val2 from pdo_fetch_all_class_ctor_error",
        )],
    );
    user_class(
        p.vm(),
        "TestBase",
        None,
        &[("id", PUBLIC), ("val", PROTECTED), ("val2", PRIVATE)],
        None,
    );
    p.m(&stmt, "execute", vec![]);
    let r = p.tm(
        &stmt,
        "fetchAll",
        vec![i(FETCH_CLASS), s("TestBase"), arr(vec![i(0)])],
    );
    if let Some(v) = p.catch(r) {
        p.dump(&v);
    }
    let r = p.tm(
        &stmt,
        "setFetchMode",
        vec![i(FETCH_CLASS), s("TestBase"), arr(vec![i(0)])],
    );
    if let Some(v) = p.catch(r) {
        p.dump(&v);
    }
    p.check(phpt!("pdo/tests/pdo_stmt_class_ctor_errors.phpt"));
}

#[test]
fn uninitialized_objects() {
    let mut p = P::new();
    user_class(
        p.vm(),
        "MyPDO",
        Some("PDO"),
        &[],
        Some((&EMPTY_CTOR, PUBLIC)),
    );
    user_class(
        p.vm(),
        "MyPDOStatement",
        Some("PDOStatement"),
        &[],
        Some((&EMPTY_CTOR, PUBLIC)),
    );
    let pdo = p.new_obj("MyPDO", vec![]);
    let r = p.tm(&pdo, "query", vec![s("foo")]);
    p.catch(r);
    let stmt = p.new_obj("MyPDOStatement", vec![]);
    let r = p.tm(&stmt, "fetch", vec![]);
    p.catch(r);
    let stmt = p.new_obj("MyPDOStatement", vec![]);
    let r = p.tm(&stmt, "getIterator", vec![]);
    p.catch(r);
    p.check(phpt!("pdo/tests/pdo_uninitialized.phpt"));
}

#[test]
fn constructor_option_argnum() {
    let mut p = P::new();
    for connect in [false, true] {
        let options = [
            iassoc(vec![(ATTR_ERRMODE, i(999))]),
            iassoc(vec![(ATTR_STATEMENT_CLASS, s("PDOStatement"))]),
        ];
        for opts in options {
            let args = vec![s("sqlite::memory:"), Value::Null, Value::Null, opts];
            let r = if connect {
                p.ts("PDO", "connect", args)
            } else {
                p.try_new("PDO", args).map(Value::Object)
            };
            p.catch(r);
        }
        let args = vec![
            s("sqlite::memory:"),
            Value::Null,
            Value::Null,
            iassoc(vec![(ATTR_DEFAULT_FETCH_MODE, i(FETCH_NUM))]),
        ];
        let db = if connect {
            p.ts("PDO", "connect", args)
                .unwrap()
                .as_object()
                .cloned()
                .unwrap()
        } else {
            p.new_obj("PDO", args)
        };
        let r = p.m(&db, "getAttribute", vec![i(ATTR_DEFAULT_FETCH_MODE)]);
        p.dump(&Value::Bool(r.as_int() == Some(FETCH_NUM)));
    }
    p.check(phpt!("pdo/tests/pdo_constructor_option_argnum.phpt"));
}

#[test]
fn oracle_nulls_invalid() {
    let mut p = P::new();
    let pdo = p.factory();
    for value in [0, 1, 2] {
        p.echo("Valid mode: ");
        let r = p.m(&pdo, "setAttribute", vec![i(ATTR_ORACLE_NULLS), i(value)]);
        p.dump(&r);
        let r = p.m(&pdo, "getAttribute", vec![i(ATTR_ORACLE_NULLS)]);
        p.dump(&r);
    }
    for value in [-1, 3, 4, i64::MAX] {
        let r = p.tm(&pdo, "setAttribute", vec![i(ATTR_ORACLE_NULLS), i(value)]);
        p.catch(r);
        p.echo("Unchanged mode: ");
        let r = p.m(&pdo, "getAttribute", vec![i(ATTR_ORACLE_NULLS)]);
        p.dump(&r);
    }
    p.check(phpt!("pdo/tests/pdo_oracle_nulls_invalid.phpt"));
}

#[test]
fn bind_column_unknown_column() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE pdo_bindcolumn_unknown_column (name varchar(255))",
    );
    let var = PhpRef::new(Value::Null);
    for mode in [0, 1, 2] {
        p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(mode)]);
        let stmt = p.obj(
            &db,
            "query",
            vec![s("SELECT name FROM pdo_bindcolumn_unknown_column")],
        );
        let r = p.tm(
            &stmt,
            "bindColumn",
            vec![s("nosuchcolumn"), Value::Ref(var.clone())],
        );
        p.catch(r);
    }
    let stmt = p.obj(
        &db,
        "query",
        vec![s("SELECT name FROM pdo_bindcolumn_unknown_column")],
    );
    let r = p.m(
        &stmt,
        "bindColumn",
        vec![s("name"), Value::Ref(var.clone())],
    );
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_bindcolumn_unknown_column.phpt"));
}

#[test]
fn quote_empty_string() {
    let mut p = P::new();
    let pdo = p.factory();
    p.m(&pdo, "setAttribute", vec![i(ATTR_ERRMODE), i(2)]);
    let r = p.m(&pdo, "quote", vec![s("")]);
    assert_eq!(r.as_str().map(|s| s.to_vec()), Some(b"''".to_vec()));
    let r = p.m(&pdo, "quote", vec![s("it's")]);
    assert_eq!(r.as_str().map(|s| s.to_vec()), Some(b"'it''s'".to_vec()));
    let r = p.tm(&pdo, "quote", vec![Value::bytes(b"a\0b")]);
    p.catch(r);
    p.m(&pdo, "setAttribute", vec![i(ATTR_ERRMODE), i(1)]);
    let r = p.m(&pdo, "quote", vec![Value::bytes(b"a\0b")]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "PDOException: SQLite PDO::quote does not support null bytes\n\nWarning: PDO::quote(): SQLite PDO::quote does not support null bytes in /t.php on line 3\nbool(false)\n"
    );
}

#[test]
fn pdorow_cannot_be_created() {
    let mut p = P::new();
    let r = p.try_new("PDORow", vec![]).map(Value::Object);
    p.catch(r);
    assert_eq!(
        p.output(),
        "PDOException: You may not create a PDORow manually\n"
    );
}

#[test]
fn pdo_drivers_basic() {
    let mut p = P::new();
    let r = p.ts("PDO", "getAvailableDrivers", vec![]).unwrap();
    p.dump(&r);
    let r = p.t.call("pdo_drivers", vec![]).unwrap();
    p.dump(&r);
    assert_eq!(
        p.output(),
        "array(1) {\n  [0]=>\n  string(6) \"sqlite\"\n}\narray(1) {\n  [0]=>\n  string(6) \"sqlite\"\n}\n"
    );
}

#[test]
fn constructor_failures() {
    let mut p = P::new();
    for (dsn, want) in [
        (
            "foo",
            "PDOException: PDO::__construct(): Argument #1 ($dsn) must be a valid data source name",
        ),
        ("nope:bar", "PDOException: could not find driver"),
        (
            "sqlite:/nonexistent-dir/x/y.db",
            "PDOException: SQLSTATE[HY000] [14] unable to open database file",
        ),
    ] {
        let r = p.try_new("PDO", vec![s(dsn)]).map(Value::Object);
        p.catch(r);
        assert_eq!(p.output().trim_end(), want);
    }
    // PDOException carries errorInfo and the driver code.
    let r = p.try_new("PDO", vec![s("sqlite:/nonexistent-dir/x/y.db")]);
    let Err(php_runtime::Flow::Exception(e)) = r else {
        panic!()
    };
    p.dump(&e.get_prop(b"errorInfo").unwrap());
    p.dump(&e.get_prop(b"code").unwrap());
    assert_eq!(
        p.output(),
        "array(3) {\n  [0]=>\n  string(5) \"HY000\"\n  [1]=>\n  int(14)\n  [2]=>\n  string(28) \"unable to open database file\"\n}\nint(14)\n"
    );
}

#[test]
fn error_modes_and_error_info() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    // ERRMODE_EXCEPTION is the default.
    let r = p.tm(&db, "exec", vec![s("SELECT * FROM foo")]);
    let Err(php_runtime::Flow::Exception(e)) = r else {
        panic!()
    };
    p.echo(format!("{}\n", Vm::exception_message(&e)));
    p.dump(&e.get_prop(b"code").unwrap());
    p.dump(&e.get_prop(b"errorInfo").unwrap());
    p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(1)]);
    let r = p.m(&db, "query", vec![s("SELECT * FROM foo")]);
    p.dump(&r);
    p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(0)]);
    let r = p.m(&db, "prepare", vec![s("SELECT * FROM foo")]);
    p.dump(&r);
    let r = p.m(&db, "errorCode", vec![]);
    p.dump(&r);
    let r = p.m(&db, "errorInfo", vec![]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "SQLSTATE[HY000]: General error: 1 no such table: foo\nstring(5) \"HY000\"\narray(3) {\n  [0]=>\n  string(5) \"HY000\"\n  [1]=>\n  int(1)\n  [2]=>\n  string(18) \"no such table: foo\"\n}\n\nWarning: PDO::query(): SQLSTATE[HY000]: General error: 1 no such table: foo in /t.php on line 3\nbool(false)\nbool(false)\nstring(5) \"HY000\"\narray(3) {\n  [0]=>\n  string(5) \"HY000\"\n  [1]=>\n  int(1)\n  [2]=>\n  string(18) \"no such table: foo\"\n}\n"
    );
    // A fresh handle has no error; an empty statement is a ValueError.
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let r = p.m(&db, "errorCode", vec![]);
    p.dump(&r);
    let r = p.tm(&db, "prepare", vec![s("")]);
    p.catch(r);
    let r = p.tm(&db, "exec", vec![s("")]);
    p.catch(r);
    assert_eq!(
        p.output(),
        "NULL\nValueError: PDO::prepare(): Argument #1 ($query) must not be empty\nValueError: PDO::exec(): Argument #1 ($statement) must not be empty\n"
    );
}

#[test]
fn transactions_errors() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let r = p.tm(&db, "commit", vec![]);
    p.catch(r);
    let r = p.tm(&db, "rollBack", vec![]);
    p.catch(r);
    let r = p.m(&db, "inTransaction", vec![]);
    p.dump(&r);
    p.m(&db, "beginTransaction", vec![]);
    let r = p.tm(&db, "beginTransaction", vec![]);
    p.catch(r);
    let r = p.m(&db, "inTransaction", vec![]);
    p.dump(&r);
    // A transaction ended by SQL is seen through sqlite3_get_autocommit().
    p.exec(&db, "COMMIT");
    let r = p.m(&db, "inTransaction", vec![]);
    p.dump(&r);
    p.exec(&db, "BEGIN");
    let r = p.m(&db, "inTransaction", vec![]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "PDOException: There is no active transaction\nPDOException: There is no active transaction\nbool(false)\nPDOException: There is already an active transaction\nbool(true)\nbool(false)\nbool(true)\n"
    );
}

fn assign_a(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = args.this().cloned().unwrap();
    let a = args.get(0).clone();
    crate::pdo::props::update_property(vm, Some(&this.class().clone()), &this, &"a".into(), a)?;
    Ok(Value::Null)
}

static FOO_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct($a)",
    handler: assign_a,
};

#[test]
fn fetch_object_ctor_args() {
    let mut p = P::new();
    let db = p.factory();
    let table = "pdo_stmt_fetchobject_ctor_args";
    p.exec(
        &db,
        &format!("CREATE TABLE {table} (id INT, label CHAR(1), PRIMARY KEY(id))"),
    );
    for (id, l) in [(1, "a"), (2, "b"), (3, "c")] {
        p.exec(
            &db,
            &format!("INSERT INTO {table} (id, label) VALUES ({id}, '{l}')"),
        );
    }
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s(&format!("SELECT id FROM {table} ORDER BY id ASC"))],
    );
    user_class_ex(
        p.vm(),
        "Foo",
        None,
        0,
        &[("a", PUBLIC, Some("int")), ("id", PUBLIC, Some("int"))],
        &[(&FOO_CTOR, PUBLIC)],
    );
    user_class_ex(p.vm(), "Bar", None, 0, &[("id", PUBLIC, Some("int"))], &[]);
    // A native constructor reports missing arguments in its own words.
    p.m(&stmt, "execute", vec![]);
    let r = p.tm(&stmt, "fetchObject", vec![s("Foo")]);
    assert!(r.is_err());
    p.m(&stmt, "execute", vec![]);
    let r = p.m(
        &stmt,
        "fetchObject",
        vec![s("Foo"), assoc(vec![("a", i(123))])],
    );
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchObject", vec![s("Bar")]);
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchObject", vec![s("Bar"), arr(vec![])]);
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let r = p.tm(
        &stmt,
        "fetchObject",
        vec![s("Bar"), assoc(vec![("a", i(123))])],
    );
    p.catch(r);
    let (_, want) = expect_section(phpt!("pdo/tests/pdo_stmt_fetchobject_ctor_args.phpt"));
    let want: String = want
        .lines()
        .filter(|l| !l.starts_with("ArgumentCountError"))
        .map(|l| format!("{l}\n"))
        .collect();
    let out = p.output();
    assert!(
        expectf_match(want.trim().as_bytes(), out.trim().as_bytes()),
        "{out}"
    );
}

fn test_derived_ctor(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = args.this().cloned().unwrap();
    let row = args.by_ref(0);
    let id = this.get_prop(b"id").unwrap_or(Value::Null);
    let r = vm.to_php_string(&row.get())?;
    let id = vm.to_php_string(&id)?;
    vm.echo(format!("TestDerived::__construct({r},{id})\n").as_bytes())?;
    let cur = row.get();
    this.set_prop(b"row", cur.clone());
    row.set(Value::Int(cur.as_int().unwrap_or(0) + 1));
    Ok(Value::Null)
}

static TEST_DERIVED_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct(&$row)",
    handler: test_derived_ctor,
};

#[test]
fn fetch_class_basic() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test005(id int NOT NULL PRIMARY KEY, val VARCHAR(10), val2 VARCHAR(10))",
    );
    for (id, v) in [(1, "A"), (2, "B"), (3, "C")] {
        p.exec(
            &db,
            &format!("INSERT INTO test005 VALUES({id}, '{v}', '{v}{v}')"),
        );
    }
    let stmt = p.obj(&db, "prepare", vec![s("SELECT id, val, val2 from test005")]);
    user_class(
        p.vm(),
        "TestBase",
        None,
        &[("id", PUBLIC), ("val", PROTECTED), ("val2", PRIVATE)],
        None,
    );
    user_class_ex(
        p.vm(),
        "TestDerived",
        Some("TestBase"),
        class_flags::ALLOW_DYNAMIC_PROPERTIES,
        &[("row", PROTECTED, None)],
        &[(&TEST_DERIVED_CTOR, PUBLIC)],
    );
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_CLASS)]);
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_CLASS), s("TestBase")]);
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let counter = PhpRef::new(i(0));
    let r = p.m(
        &stmt,
        "fetchAll",
        vec![
            i(FETCH_CLASS),
            s("TestDerived"),
            arr(vec![Value::Ref(counter)]),
        ],
    );
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_fetch_class_basic.phpt"));
}

fn associate_val_with_id(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (id, val) = (args.get(0).clone(), args.get(1).clone());
    vm.echo(format!("associateValWithId({}, {})\n", export(&id), export(&val)).as_bytes())?;
    let mut a = php_runtime::PhpArray::new();
    a.insert(php_runtime::ArrayKey::Int(args.int(0)), val);
    Ok(Value::array(a))
}

fn select_all_callback(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (id, v1, v2) = (
        args.get(0).clone(),
        args.get(1).clone(),
        args.get(2).clone(),
    );
    vm.echo(
        format!(
            "selectAllCallback({}, {}, {})\n",
            export(&id),
            export(&v1),
            export(&v2)
        )
        .as_bytes(),
    )?;
    let mut out = args.str(1).to_vec();
    out.extend_from_slice(args.str(2));
    Ok(Value::from(out))
}

fn bogus_callback(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.echo(b"Called\n")?;
    Ok(s("how?"))
}

static ASSOCIATE: NativeFunction = NativeFunction {
    sig: "associateValWithId(int $id, string $val): array",
    handler: associate_val_with_id,
};
static SELECT_ALL: NativeFunction = NativeFunction {
    sig: "selectAllCallback(int $id, string $val1, string $val2): string",
    handler: select_all_callback,
};
static BOGUS: NativeFunction = NativeFunction {
    sig: "bogusCallback(stdClass $obj, string $str, array $arr = [])",
    handler: bogus_callback,
};

#[test]
fn fetch_function_basic() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "CREATE TABLE pdo_fetch_function_basic(id int NOT NULL PRIMARY KEY, val1 VARCHAR(10), val2 VARCHAR(10))");
    for (id, a, b) in [
        (1, "A", "alpha"),
        (2, "B", "beta"),
        (3, "C", "gamma"),
        (4, "D", "delta"),
    ] {
        p.exec(
            &db,
            &format!("INSERT INTO pdo_fetch_function_basic VALUES ({id}, '{a}', '{b}')"),
        );
    }
    user_function(p.vm(), &ASSOCIATE);
    user_function(p.vm(), &SELECT_ALL);
    p.echo("SELECT id, val1:\n");
    let st = p.obj(
        &db,
        "prepare",
        vec![s("SELECT id, val1 FROM pdo_fetch_function_basic")],
    );
    p.m(&st, "execute", vec![]);
    let r = p.m(
        &st,
        "fetchAll",
        vec![i(FETCH_FUNC), s("associateValWithId")],
    );
    p.dump(&r);
    p.echo("SELECT *:\n");
    let st = p.obj(
        &db,
        "prepare",
        vec![s("SELECT * FROM pdo_fetch_function_basic")],
    );
    p.m(&st, "execute", vec![]);
    let r = p.m(&st, "fetchAll", vec![i(FETCH_FUNC), s("selectAllCallback")]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_fetch_function_basic.phpt"));
}

#[test]
fn fetch_function_incorrect_call() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "CREATE TABLE pdo_fetch_function_incorrect_call(id int NOT NULL PRIMARY KEY, val1 VARCHAR(10), val2 VARCHAR(10))");
    p.exec(
        &db,
        "INSERT INTO pdo_fetch_function_incorrect_call VALUES(1, 'A', 'alpha')",
    );
    user_function(p.vm(), &BOGUS);
    let st = p.obj(
        &db,
        "prepare",
        vec![s(
            "SELECT val1, val2 FROM pdo_fetch_function_incorrect_call",
        )],
    );
    p.m(&st, "execute", vec![]);
    p.echo("Fetch all with bogus call:\n");
    let r = p.tm(&st, "fetchAll", vec![i(FETCH_FUNC), s("bogusCallback")]);
    p.catch(r);
    p.check(phpt!("pdo/tests/pdo_fetch_function_incorrect_call.phpt"));
}

#[test]
fn fetch_func_with_internal_function_and_grouping() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE f(id int NOT NULL PRIMARY KEY, pl_name VARCHAR(10), grp VARCHAR(10))",
    );
    p.exec(&db, "INSERT INTO f VALUES (1, 'php', 'g1')");
    p.exec(&db, "INSERT INTO f VALUES (2, 'sql', 'g1')");
    let st = p.obj(&db, "query", vec![s("SELECT pl_name FROM f")]);
    let r = p.m(&st, "fetchAll", vec![i(FETCH_FUNC), s("strtoupper")]);
    p.dump(&r);
    let st = p.obj(&db, "query", vec![s("SELECT grp, pl_name FROM f")]);
    let r = p.m(
        &st,
        "fetchAll",
        vec![i(FETCH_FUNC | FETCH_GROUP), s("strtoupper")],
    );
    p.dump(&r);
    let st = p.obj(&db, "query", vec![s("SELECT pl_name FROM f")]);
    let r = p.tm(&st, "fetchAll", vec![i(FETCH_FUNC), s("no_such_function")]);
    p.catch(r);
    let r = p.tm(&st, "fetchAll", vec![i(FETCH_FUNC)]);
    p.catch(r);
    let r = p.tm(&st, "fetch", vec![i(FETCH_FUNC)]);
    p.catch(r);
    assert_eq!(
        p.output(),
        "array(2) {\n  [0]=>\n  string(3) \"PHP\"\n  [1]=>\n  string(3) \"SQL\"\n}\narray(1) {\n  [\"g1\"]=>\n  array(2) {\n    [0]=>\n    string(3) \"PHP\"\n    [1]=>\n    string(3) \"SQL\"\n  }\n}\nTypeError: function \"no_such_function\" not found or invalid function name\nArgumentCountError: PDOStatement::fetchAll() expects exactly 2 argument for PDO::FETCH_FUNC, 1 given\nValueError: PDOStatement::fetch(): Argument #1 ($mode) PDO::FETCH_FUNC can only be used with PDOStatement::fetchAll()\n"
    );
}

/// `class PDOException extends RuntimeException` (pdo.stub.php): SPL is
/// registered before PDO, so the real parent must be found.
#[test]
fn pdo_exception_extends_runtime_exception() {
    let t = crate::testing::TestVm::new();
    let ce = t.vm.find_class(b"PDOException").expect("PDOException");
    let parent = ce.parent.as_ref().expect("PDOException has a parent");
    assert_eq!(parent.name.as_bytes(), b"RuntimeException");
}
