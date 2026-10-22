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
