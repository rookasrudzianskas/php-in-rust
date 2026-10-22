//! Statements: execution with parameters, binding, lazy rows, FETCH_INTO,
//! metadata and `debugDumpParams()`.

use php_runtime::{PhpRef, Value};

use super::harness::*;
use super::phpt;
use crate::pdo::consts::*;

const DATA: [[&str; 3]; 6] = [
    ["10", "Abc", "zxy"],
    ["20", "Def", "wvu"],
    ["30", "Ghi", "tsr"],
    ["40", "Jkl", "qpo"],
    ["50", "Mno", "nml"],
    ["60", "Pqr", "kji"],
];

fn insert_data(p: &mut P, db: &php_runtime::ObjRef, table: &str) {
    let stmt = p.obj(
        db,
        "prepare",
        vec![s(&format!("INSERT INTO {table} VALUES(?, ?, ?)"))],
    );
    for row in DATA {
        p.m(
            &stmt,
            "execute",
            vec![arr(row.iter().map(|v| s(v)).collect())],
        );
    }
}

#[test]
fn pdo_021_execute_with_parameters() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test021(id INT NOT NULL PRIMARY KEY, val VARCHAR(10), val2 VARCHAR(16))",
    );
    let select = p.obj(&db, "prepare", vec![s("SELECT COUNT(id) FROM test021")]);
    insert_data(&mut p, &db, "test021");
    p.m(&select, "execute", vec![]);
    let num = p.m(&select, "fetchColumn", vec![]);
    p.echo(format!(
        "There are {} rows in the table.\n",
        num.as_str().unwrap()
    ));
    let stmt2 = p.obj(
        &db,
        "prepare",
        vec![s("INSERT INTO test021 VALUES(:first, :second, :third)")],
    );
    for row in DATA {
        let first: i64 = row[0].parse::<i64>().unwrap() + 5;
        p.m(
            &stmt2,
            "execute",
            vec![assoc(vec![
                (":first", i(first)),
                (":second", s(row[1])),
                (":third", s(row[2])),
            ])],
        );
    }
    p.m(&select, "execute", vec![]);
    let num = p.m(&select, "fetchColumn", vec![]);
    p.echo(format!(
        "There are {} rows in the table.\n",
        num.as_str().unwrap()
    ));
    p.check(phpt!("pdo/tests/pdo_021.phpt"));
}

#[test]
fn pdo_024_bind_param_keeps_null() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "create table test024 (id int, name varchar(10))");
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s("insert into test024 (id, name) values(0, :name)")],
    );
    let name = PhpRef::new(Value::Null);
    p.m(
        &stmt,
        "bindParam",
        vec![s(":name"), Value::Ref(name.clone()), i(PARAM_NULL)],
    );
    p.echo(if name.get().is_null() {
        "bind: success\n"
    } else {
        "bind: fail\n"
    });
    let r = p.m(&stmt, "execute", vec![]);
    p.dump(&r);
    let q = p.obj(&db, "query", vec![s("select name from test024 where id=0")]);
    let r = p.m(&q, "fetchColumn", vec![]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_024.phpt"));
}

#[test]
fn pdo_025_fetch_into() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test025(id INT NOT NULL PRIMARY KEY, val VARCHAR(10), val2 VARCHAR(16))",
    );
    insert_data(&mut p, &db, "test025");
    user_class(
        p.vm(),
        "Test",
        None,
        &[("id", PUBLIC), ("val", PUBLIC), ("val2", PUBLIC)],
        None,
    );
    let stmt = p.obj(&db, "prepare", vec![s("SELECT * FROM test025")]);
    let test = p.new_obj("Test", vec![]);
    p.m(
        &stmt,
        "setFetchMode",
        vec![i(FETCH_INTO), Value::Object(test)],
    );
    p.m(&stmt, "execute", vec![]);
    let it = p.obj(&stmt, "getIterator", vec![]);
    while matches!(p.m(&it, "valid", vec![]), Value::Bool(true)) {
        let v = p.m(&it, "current", vec![]);
        p.dump(&v);
        p.m(&it, "next", vec![]);
    }
    p.echo("===FAIL===\n");
    user_class(
        p.vm(),
        "Fail",
        None,
        &[("id", PROTECTED), ("val", PUBLIC), ("val2", PUBLIC)],
        None,
    );
    let fail = p.new_obj("Fail", vec![]);
    p.m(
        &stmt,
        "setFetchMode",
        vec![i(FETCH_INTO), Value::Object(fail)],
    );
    p.m(&stmt, "execute", vec![]);
    let r = p.tm(&stmt, "getIterator", vec![]);
    let (ty, msg) = crate::testing::thrown(r);
    p.echo(format!(
        "\nFatal error: Uncaught {ty}: {msg} in /tmp/pdo_025.php:1\nStack trace:\n#0 {{main}}\n  thrown in /tmp/pdo_025.php on line 1\n"
    ));
    p.check(phpt!("pdo/tests/pdo_025.phpt"));
}

#[test]
fn pdo_027_fetch_lazy() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "create table test027 (id int, name varchar(10))");
    p.exec(&db, "INSERT INTO test027 (id,name) VALUES(1,'test1')");
    p.exec(&db, "INSERT INTO test027 (id,name) VALUES(2,'test2')");
    let r = p.obj(
        &db,
        "query",
        vec![s("SELECT * FROM test027"), i(FETCH_LAZY)],
    );
    p.dump(&Value::Object(r.clone()));
    let it = p.obj(&r, "getIterator", vec![]);
    let mut alias: Option<php_runtime::ObjRef> = None;
    while matches!(p.m(&it, "valid", vec![]), Value::Bool(true)) {
        let v = p.m(&it, "current", vec![]);
        let o = v.as_object().cloned().unwrap();
        p.dump(&Value::Bool(
            alias
                .as_ref()
                .is_some_and(|a| php_runtime::ObjRef::ptr_eq(a, &o)),
        ));
        p.dump(&v);
        alias = Some(o.clone());
        let read = o.class().handlers.read_property.unwrap();
        let id = read(p.vm(), &o, &"id".into()).unwrap().unwrap();
        let name = read(p.vm(), &o, &"name".into()).unwrap().unwrap();
        p.echo(format!(
            "lazy: {}{}\n",
            id.as_str().unwrap(),
            name.as_str().unwrap()
        ));
        p.m(&it, "next", vec![]);
    }
    p.echo("End\n");
    p.check(phpt!("pdo/tests/pdo_027.phpt"));
}

#[test]
fn pdo_028_bind_value() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(&db, "CREATE TABLE test028(id int NOT NULL PRIMARY KEY, val1 VARCHAR(10), val2 VARCHAR(10), val3 VARCHAR(10))");
    let stmt = p.obj(
        &db,
        "prepare",
        vec![s("INSERT INTO test028 values (1, ?, ?, ?)")],
    );
    for (n, v) in ["one", "two", "three"].iter().enumerate() {
        p.m(&stmt, "bindValue", vec![i(n as i64 + 1), s(v)]);
    }
    p.m(&stmt, "execute", vec![]);
    let stmt = p.obj(&db, "prepare", vec![s("SELECT * from test028")]);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_ASSOC)]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_028.phpt"));
}

#[test]
fn debug_dump_params_with_bound_params() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let x = p.obj(&db, "prepare", vec![s("select :a, :b, ?")]);
    p.m(&x, "bindValue", vec![s(":a"), i(1), i(PARAM_INT)]);
    p.m(&x, "bindValue", vec![s(":b"), s("foo")]);
    p.m(&x, "bindValue", vec![i(3), i(1313)]);
    let r = p.m(&x, "debugDumpParams", vec![]);
    p.dump(&r);
    p.check(phpt!("pdo_sqlite/tests/debugdumpparams_001.phpt"));
}

/// `pdo_parse_params` as used by drivers without native placeholders
/// (emulated prepares) and by drivers with the other placeholder style.
#[test]
fn emulated_prepare_parsing() {
    use crate::pdo::parser::{Parsed, parse_params};
    use crate::pdo::stmt::{
        BoundParam, PLACEHOLDER_NAMED, PLACEHOLDER_NONE, PLACEHOLDER_POSITIONAL, ParamKey,
        ParamTable, Stmt,
    };
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let dbh = crate::pdo::dbh::dbh_of(&db).unwrap();
    let param = |v: Value, ty: i64| BoundParam {
        parameter: v,
        driver_params: Value::Undef,
        paramno: -1,
        name: None,
        max_value_len: 0,
        param_type: ty as i32,
        is_param: true,
    };
    let run = |p: &mut P, supports: u8, sql: &str, params: Vec<(ParamKey, BoundParam)>| {
        let stmt = Stmt::new();
        stmt.supports_placeholders.set(supports);
        *stmt.dbh.borrow_mut() = Some(dbh.clone());
        if !params.is_empty() {
            let mut t = ParamTable::default();
            for (k, v) in params {
                t.update(k, v);
            }
            *stmt.bound_params.borrow_mut() = Some(t);
        }
        let r = match parse_params(p.vm(), &stmt, &dbh, &sql.into()) {
            Ok(Parsed::Unchanged) => "unchanged".to_string(),
            Ok(Parsed::Rewritten(q)) => q.to_string_lossy().into_owned(),
            Ok(Parsed::Error) => format!(
                "error {}",
                crate::pdo::error::err_str(&stmt.error_code.get())
            ),
            Err(e) => describe(&e),
        };
        let map = stmt.bound_param_map.borrow().clone().unwrap_or_default();
        (r, map)
    };
    let (r, _) = run(
        &mut p,
        PLACEHOLDER_NONE,
        "SELECT ?, ?, '?', ?",
        vec![
            (ParamKey::Int(0), param(s("a'b"), PARAM_STR)),
            (ParamKey::Int(1), param(s("12abc"), PARAM_INT)),
            (ParamKey::Int(2), param(Value::Null, PARAM_STR)),
        ],
    );
    assert_eq!(r, "SELECT 'a''b', 12, '?', NULL");
    let (r, _) = run(
        &mut p,
        PLACEHOLDER_NONE,
        "SELECT :a, :b, :a",
        vec![
            (
                ParamKey::Str(":a".into()),
                param(Value::Bool(true), PARAM_BOOL),
            ),
            (ParamKey::Str(":b".into()), param(i(7), PARAM_STR)),
        ],
    );
    assert_eq!(r, "SELECT 1, '7', 1");
    let (r, _) = run(&mut p, PLACEHOLDER_NONE, "SELECT 1", vec![]);
    assert_eq!(r, "unchanged");
    // Mixed styles and wrong counts are HY093 (ERRMODE_EXCEPTION).
    let (r, _) = run(
        &mut p,
        PLACEHOLDER_NONE,
        "SELECT :a, ?",
        vec![(ParamKey::Int(0), param(i(1), PARAM_INT))],
    );
    assert_eq!(
        r,
        "PDOException: SQLSTATE[HY093]: Invalid parameter number: mixed named and positional parameters"
    );
    let (r, _) = run(
        &mut p,
        PLACEHOLDER_NONE,
        "SELECT ?, ?",
        vec![(ParamKey::Int(0), param(i(1), PARAM_INT))],
    );
    assert_eq!(
        r,
        "PDOException: SQLSTATE[HY093]: Invalid parameter number: number of bound variables does not match number of tokens"
    );
    let (r, _) = run(
        &mut p,
        PLACEHOLDER_NONE,
        "SELECT :a",
        vec![(ParamKey::Str(":b".into()), param(i(1), PARAM_INT))],
    );
    assert_eq!(
        r,
        "PDOException: SQLSTATE[HY093]: Invalid parameter number: parameter was not defined"
    );
    // `?` for a named-only driver becomes `:pdoN`.
    let (r, map) = run(&mut p, PLACEHOLDER_NAMED, "SELECT ?, ?", vec![]);
    assert_eq!(r, "SELECT :pdo1, :pdo2");
    assert_eq!(
        map,
        vec![
            (ParamKey::Int(0), ":pdo1".into()),
            (ParamKey::Int(1), ":pdo2".into())
        ]
    );
    // `:name` for a positional-only driver becomes `?`.
    let (r, map) = run(&mut p, PLACEHOLDER_POSITIONAL, "SELECT :x, :y", vec![]);
    assert_eq!(r, "SELECT ?, ?");
    assert_eq!(
        map,
        vec![
            (ParamKey::Int(0), ":x".into()),
            (ParamKey::Int(1), ":y".into())
        ]
    );
    // A `:name` glued to an identifier is not a placeholder.
    let (r, _) = run(&mut p, PLACEHOLDER_POSITIONAL, "SELECT a:b, :c", vec![]);
    assert_eq!(r, "SELECT a:b, ?");
}

#[test]
fn fetch_named_key_pair_and_oracle_nulls() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let st = p.obj(
        &db,
        "query",
        vec![s("SELECT 1 AS a, 2 AS a, '' AS b, NULL AS c")],
    );
    let r = p.m(&st, "fetch", vec![i(FETCH_NAMED)]);
    p.dump(&r);
    let st = p.obj(
        &db,
        "query",
        vec![s("SELECT 'k1', 'v1' UNION ALL SELECT 'k2', 'v2'")],
    );
    let r = p.m(&st, "fetchAll", vec![i(FETCH_KEY_PAIR)]);
    p.dump(&r);
    // pdo_raise_impl_error() warns, then PDO_HANDLE_STMT_ERR() warns again.
    p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(1)]);
    let st = p.obj(&db, "query", vec![s("SELECT 1, 2, 3")]);
    let r = p.m(&st, "fetch", vec![i(FETCH_KEY_PAIR)]);
    p.dump(&r);
    p.m(&db, "setAttribute", vec![i(ATTR_ORACLE_NULLS), i(1)]);
    let st = p.obj(&db, "query", vec![s("SELECT '' AS b, NULL AS c")]);
    let r = p.m(&st, "fetch", vec![i(FETCH_ASSOC)]);
    p.dump(&r);
    p.m(&db, "setAttribute", vec![i(ATTR_ORACLE_NULLS), i(2)]);
    let st = p.obj(&db, "query", vec![s("SELECT '' AS b, NULL AS c")]);
    let r = p.m(&st, "fetch", vec![i(FETCH_ASSOC)]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "array(3) {\n  [\"a\"]=>\n  array(2) {\n    [0]=>\n    int(1)\n    [1]=>\n    int(2)\n  }\n  [\"b\"]=>\n  string(0) \"\"\n  [\"c\"]=>\n  NULL\n}\n\
         array(2) {\n  [\"k1\"]=>\n  string(2) \"v1\"\n  [\"k2\"]=>\n  string(2) \"v2\"\n}\n\
         \nWarning: PDOStatement::fetch(): SQLSTATE[HY000]: General error: PDO::FETCH_KEY_PAIR fetch mode requires the result set to contain exactly 2 columns. in /t.php on line 3\n\nWarning: PDOStatement::fetch(): SQLSTATE[HY000]: General error in /t.php on line 3\nbool(false)\n\
         array(2) {\n  [\"b\"]=>\n  NULL\n  [\"c\"]=>\n  NULL\n}\n\
         array(2) {\n  [\"b\"]=>\n  string(0) \"\"\n  [\"c\"]=>\n  string(0) \"\"\n}\n"
    );
}

#[test]
fn statement_class_attribute() {
    let mut p = P::new();
    user_class(
        p.vm(),
        "MyStmt",
        Some("PDOStatement"),
        &[],
        Some((&ECHO_CTOR_NAME, PROTECTED)),
    );
    user_class(
        p.vm(),
        "PublicCtorStmt",
        Some("PDOStatement"),
        &[],
        Some((&ECHO_CTOR_NAME, PUBLIC)),
    );
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let r = p.m(&db, "getAttribute", vec![i(ATTR_STATEMENT_CLASS)]);
    p.dump(&r);
    p.m(
        &db,
        "setAttribute",
        vec![
            i(ATTR_STATEMENT_CLASS),
            arr(vec![s("MyStmt"), arr(vec![s("x")])]),
        ],
    );
    let r = p.m(&db, "getAttribute", vec![i(ATTR_STATEMENT_CLASS)]);
    p.dump(&r);
    let st = p.obj(&db, "query", vec![s("SELECT 1")]);
    p.echo(format!("{}\n", st.class().name));
    let r = p.m(&st, "fetchColumn", vec![]);
    p.dump(&r);
    for bad in [
        s("MyStmt"),
        arr(vec![]),
        arr(vec![s("NoSuchClass")]),
        arr(vec![s("stdClass")]),
        arr(vec![s("PublicCtorStmt")]),
        arr(vec![s("MyStmt"), s("x")]),
    ] {
        let r = p.tm(&db, "setAttribute", vec![i(ATTR_STATEMENT_CLASS), bad]);
        p.catch(r);
    }
    // Per-statement class through prepare() options.
    let opts = iassoc(vec![(
        ATTR_STATEMENT_CLASS,
        arr(vec![s("MyStmt"), arr(vec![s("y")])]),
    )]);
    let st = p.obj(&db, "prepare", vec![s("SELECT 2"), opts]);
    let qs = st.get_prop(b"queryString").unwrap();
    p.dump(&qs);
    let r = p.tm(&st, "__construct", vec![]);
    assert!(r.is_ok());
    let opts = iassoc(vec![(ATTR_STATEMENT_CLASS, s("MyStmt"))]);
    let r = p.tm(&db, "prepare", vec![s("SELECT 2"), opts]);
    p.catch(r);
    assert_eq!(
        p.output(),
        "array(1) {\n  [0]=>\n  string(12) \"PDOStatement\"\n}\n\
         array(2) {\n  [0]=>\n  string(6) \"MyStmt\"\n  [1]=>\n  array(1) {\n    [0]=>\n    string(1) \"x\"\n  }\n}\n\
         MyStmt::__construct(x)\nMyStmt\nint(1)\n\
         TypeError: PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS value must be of type array, string given\n\
         ValueError: PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS value must be an array with the format array(classname, constructor_args)\n\
         TypeError: PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS class must be a valid class\n\
         TypeError: PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS class must be derived from PDOStatement\n\
         TypeError: PDO::setAttribute(): Argument #2 ($value) User-supplied statement class cannot have a public constructor\n\
         TypeError: PDO::setAttribute(): Argument #2 ($value) PDO::ATTR_STATEMENT_CLASS constructor_args must be of type ?array, string given\n\
         MyStmt::__construct(y)\nstring(8) \"SELECT 2\"\nMyStmt::__construct(N/A)\n\
         TypeError: PDO::ATTR_STATEMENT_CLASS value must be of type array, string given\n"
    );
}

#[test]
fn row_count_close_cursor_and_unsupported_features() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    p.exec(&db, "CREATE TABLE t (a)");
    let st = p.obj(
        &db,
        "prepare",
        vec![s("INSERT INTO t VALUES (1), (2), (3)")],
    );
    p.m(&st, "execute", vec![]);
    let r = p.m(&st, "rowCount", vec![]);
    p.dump(&r);
    let st = p.obj(&db, "query", vec![s("SELECT a FROM t")]);
    let r = p.m(&st, "fetchColumn", vec![]);
    p.dump(&r);
    let r = p.m(&st, "closeCursor", vec![]);
    p.dump(&r);
    p.m(&st, "execute", vec![]);
    let r = p.m(&st, "fetchAll", vec![i(FETCH_COLUMN)]);
    p.dump(&r);
    p.m(&db, "setAttribute", vec![i(ATTR_ERRMODE), i(0)]);
    let r = p.m(&st, "nextRowset", vec![]);
    p.dump(&r);
    let r = p.m(&st, "errorInfo", vec![]);
    p.dump(&r);
    let r = p.m(&db, "getAttribute", vec![i(ATTR_PREFETCH)]);
    p.dump(&r);
    let r = p.m(&db, "errorInfo", vec![]);
    p.dump(&r);
    let r = p.m(&st, "getAttribute", vec![i(ATTR_EMULATE_PREPARES)]);
    p.dump(&r);
    assert_eq!(
        p.output(),
        "int(3)\nint(1)\nbool(true)\narray(3) {\n  [0]=>\n  int(1)\n  [1]=>\n  int(2)\n  [2]=>\n  int(3)\n}\n\
         bool(false)\narray(3) {\n  [0]=>\n  string(5) \"IM001\"\n  [1]=>\n  NULL\n  [2]=>\n  NULL\n}\n\
         bool(false)\narray(3) {\n  [0]=>\n  string(5) \"IM001\"\n  [1]=>\n  NULL\n  [2]=>\n  NULL\n}\n\
         bool(false)\n"
    );
}

#[test]
fn bind_param_by_reference_and_types() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let st = p.obj(&db, "prepare", vec![s("SELECT :v, typeof(:v)")]);
    let v = PhpRef::new(s("12abc"));
    p.m(
        &st,
        "bindParam",
        vec![s(":v"), Value::Ref(v.clone()), i(PARAM_INT)],
    );
    p.m(&st, "execute", vec![]);
    let r = p.m(&st, "fetch", vec![i(FETCH_NUM)]);
    p.dump(&r);
    // convert_to_long() changed the bound variable itself.
    p.dump(&v.get());
    v.set(Value::Bool(true));
    p.m(&st, "execute", vec![]);
    let r = p.m(&st, "fetch", vec![i(FETCH_NUM)]);
    p.dump(&r);
    let st = p.obj(
        &db,
        "prepare",
        vec![s("SELECT typeof(?), typeof(?), typeof(?)")],
    );
    p.m(&st, "bindValue", vec![i(1), s("x"), i(PARAM_LOB)]);
    p.m(&st, "bindValue", vec![i(2), Value::Null, i(PARAM_STR)]);
    p.m(
        &st,
        "bindValue",
        vec![i(3), Value::Float(1.5), i(PARAM_STR)],
    );
    p.m(&st, "execute", vec![]);
    let r = p.m(&st, "fetch", vec![i(FETCH_NUM)]);
    p.dump(&r);
    let r = p.tm(&st, "bindValue", vec![i(0), i(1)]);
    p.catch(r);
    let r = p.tm(&st, "bindValue", vec![s(""), i(1)]);
    p.catch(r);
    assert_eq!(
        p.output(),
        "array(2) {\n  [0]=>\n  int(12)\n  [1]=>\n  string(7) \"integer\"\n}\nint(12)\n\
         array(2) {\n  [0]=>\n  int(1)\n  [1]=>\n  string(7) \"integer\"\n}\n\
         array(3) {\n  [0]=>\n  string(4) \"blob\"\n  [1]=>\n  string(4) \"null\"\n  [2]=>\n  string(4) \"text\"\n}\n\
         ValueError: PDOStatement::bindValue(): Argument #1 ($param) must be greater than or equal to 1\n\
         ValueError: PDOStatement::bindValue(): Argument #1 ($param) must not be empty\n"
    );
}

#[test]
fn pdorow_reads_and_refuses_writes() {
    let mut p = P::new();
    let db = p.new_obj("PDO", vec![s("sqlite::memory:")]);
    let st = p.obj(&db, "query", vec![s("SELECT 1 AS id, 'x' AS name")]);
    let row = p.obj(&st, "fetch", vec![i(FETCH_LAZY)]);
    let h = row.class().handlers;
    let read_dim = h.read_dimension.unwrap();
    let r = read_dim(
        p.vm(),
        &row,
        Some(&i(1)),
        php_runtime::class::DimFetch::Read,
    )
    .unwrap();
    p.dump(&r);
    let r = read_dim(
        p.vm(),
        &row,
        Some(&s("id")),
        php_runtime::class::DimFetch::Read,
    )
    .unwrap();
    p.dump(&r);
    let r = read_dim(
        p.vm(),
        &row,
        Some(&s("nope")),
        php_runtime::class::DimFetch::Read,
    )
    .unwrap();
    p.dump(&r);
    let has = h.has_dimension.unwrap();
    for (k, empty) in [(i(0), false), (i(5), false), (s("name"), true)] {
        let r = has(p.vm(), &row, &k, empty).unwrap();
        p.dump(&Value::Bool(r));
    }
    let r = (h.write_property.unwrap())(p.vm(), &row, &"id".into(), i(2)).map(|_| Value::Null);
    p.catch(r);
    let r = (h.write_dimension.unwrap())(p.vm(), &row, None, i(2)).map(|_| Value::Null);
    p.catch(r);
    let r = (h.unset_dimension.unwrap())(p.vm(), &row, &i(0)).map(|_| Value::Null);
    p.catch(r);
    assert_eq!(
        p.output(),
        "string(1) \"x\"\nint(1)\nNULL\nbool(true)\nbool(false)\nbool(true)\n\
         Error: Cannot write to PDORow property\nError: Cannot append to PDORow offset\nError: Cannot unset PDORow offset\n"
    );
}
