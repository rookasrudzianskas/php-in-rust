//! Fetch modes (`pdo_001`..`pdo_020`, FETCH_CLASS/COLUMN/BOUND/...).

use php_runtime::{ObjRef, Value};

use super::harness::*;
use super::phpt;
use crate::pdo::consts::*;

/// The three-row `id, val` table of `pdo_001`..`pdo_004`.
fn abc_table(p: &mut P, db: &ObjRef, table: &str) {
    p.exec(
        db,
        &format!("CREATE TABLE {table}(id int NOT NULL PRIMARY KEY, val VARCHAR(10))"),
    );
    for (id, v) in [(1, "A"), (2, "B"), (3, "C")] {
        p.exec(db, &format!("INSERT INTO {table} VALUES({id}, '{v}')"));
    }
}

fn fetch_all_test(mode: i64, file: &str, src: &str) {
    let mut p = P::new();
    let db = p.factory();
    abc_table(&mut p, &db, file);
    let stmt = p.obj(&db, "prepare", vec![s(&format!("SELECT * from {file}"))]);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(mode)]);
    p.dump(&r);
    p.check(src);
}

#[test]
fn pdo_001_fetch_assoc() {
    fetch_all_test(FETCH_ASSOC, "test001", phpt!("pdo/tests/pdo_001.phpt"));
}

#[test]
fn pdo_002_fetch_num() {
    fetch_all_test(FETCH_NUM, "test002", phpt!("pdo/tests/pdo_002.phpt"));
}

#[test]
fn pdo_003_fetch_both() {
    fetch_all_test(FETCH_BOTH, "test003", phpt!("pdo/tests/pdo_003.phpt"));
}

#[test]
fn pdo_004_fetch_obj() {
    fetch_all_test(FETCH_OBJ, "test004", phpt!("pdo/tests/pdo_004.phpt"));
}

#[test]
fn pdo_006_fetch_group() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test006(id int NOT NULL PRIMARY KEY, val VARCHAR(10))",
    );
    p.exec(&db, "INSERT INTO test006 VALUES(1, 'A')");
    p.exec(&db, "INSERT INTO test006 VALUES(2, 'A')");
    p.exec(&db, "INSERT INTO test006 VALUES(3, 'C')");
    let stmt = p.obj(&db, "prepare", vec![s("SELECT val, id from test006")]);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_NUM | FETCH_GROUP)]);
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_ASSOC | FETCH_GROUP)]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_006.phpt"));
}

fn unique_table(p: &mut P, db: &ObjRef, table: &str) {
    p.exec(
        db,
        &format!("CREATE TABLE {table}(id CHAR(1) NOT NULL PRIMARY KEY, val VARCHAR(10))"),
    );
    p.exec(db, &format!("INSERT INTO {table} VALUES('A', 'A')"));
    p.exec(db, &format!("INSERT INTO {table} VALUES('B', 'A')"));
    p.exec(db, &format!("INSERT INTO {table} VALUES('C', 'C')"));
}

#[test]
fn pdo_007_fetch_unique() {
    let mut p = P::new();
    let db = p.factory();
    unique_table(&mut p, &db, "test007");
    let stmt = p.obj(&db, "prepare", vec![s("SELECT id, val from test007")]);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_NUM | FETCH_UNIQUE)]);
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_ASSOC | FETCH_UNIQUE)]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_007.phpt"));
}

#[test]
fn pdo_008_fetch_unique_conflict() {
    let mut p = P::new();
    let db = p.factory();
    unique_table(&mut p, &db, "test008");
    let stmt = p.obj(&db, "prepare", vec![s("SELECT val, id from test008")]);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(&stmt, "fetchAll", vec![i(FETCH_NUM | FETCH_UNIQUE)]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_008.phpt"));
}

#[test]
fn pdo_010_classtype_group_unique() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE classtypes010(id int NOT NULL PRIMARY KEY, name VARCHAR(10) NOT NULL UNIQUE)",
    );
    p.exec(&db, "INSERT INTO classtypes010 VALUES(0, 'stdClass')");
    p.exec(&db, "INSERT INTO classtypes010 VALUES(1, 'Test1')");
    p.exec(&db, "INSERT INTO classtypes010 VALUES(2, 'Test2')");
    p.exec(&db, "CREATE TABLE test010(id int NOT NULL PRIMARY KEY, classtype int, val VARCHAR(10), grp VARCHAR(10))");
    p.exec(&db, "INSERT INTO test010 VALUES(1, 0, 'A', 'Group1')");
    p.exec(&db, "INSERT INTO test010 VALUES(2, 1, 'B', 'Group1')");
    p.exec(&db, "INSERT INTO test010 VALUES(3, 2, 'C', 'Group2')");
    p.exec(&db, "INSERT INTO test010 VALUES(4, 3, 'D', 'Group2')");
    let stmt = p.obj(&db, "prepare", vec![s("SELECT classtypes010.name, test010.grp AS grp, test010.id AS id, test010.val AS val FROM test010 LEFT JOIN classtypes010 ON test010.classtype=classtypes010.id")]);
    for name in ["Test1", "Test2", "Test3"] {
        user_class(
            p.vm(),
            name,
            None,
            &[("id", PUBLIC), ("val", PUBLIC)],
            Some((&ECHO_CTOR0, PUBLIC)),
        );
    }
    p.m(&stmt, "execute", vec![]);
    let r = p.m(
        &stmt,
        "fetchAll",
        vec![i(FETCH_CLASS | FETCH_CLASSTYPE | FETCH_GROUP), s("Test3")],
    );
    p.dump(&r);
    p.m(&stmt, "execute", vec![]);
    let r = p.m(
        &stmt,
        "fetchAll",
        vec![i(FETCH_CLASS | FETCH_CLASSTYPE | FETCH_UNIQUE), s("Test3")],
    );
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_010.phpt"));
}

fn group_table(p: &mut P, db: &ObjRef, table: &str) {
    p.exec(
        db,
        &format!(
            "CREATE TABLE {table}(id int NOT NULL PRIMARY KEY, val VARCHAR(10), grp VARCHAR(10))"
        ),
    );
    p.exec(db, &format!("INSERT INTO {table} VALUES(1, 'A', 'Group1')"));
    p.exec(db, &format!("INSERT INTO {table} VALUES(2, 'B', 'Group2')"));
}

#[test]
fn pdo_012_set_fetch_mode() {
    let mut p = P::new();
    let db = p.factory();
    group_table(&mut p, &db, "test012");
    let select = "SELECT val, grp FROM test012";
    let stmt = p.obj(&db, "query", vec![s(select), i(FETCH_NUM)]);
    let r = p.m(&stmt, "fetchAll", vec![]);
    p.dump(&r);
    user_class(
        p.vm(),
        "TestClass",
        None,
        &[("val", PUBLIC), ("grp", PUBLIC)],
        Some((&ECHO_CTOR_NAME, PUBLIC)),
    );
    drop(stmt);
    let stmt = p.obj(
        &db,
        "query",
        vec![s(select), i(FETCH_CLASS), s("TestClass")],
    );
    let r = p.m(&stmt, "fetchAll", vec![]);
    p.dump(&r);
    drop(stmt);
    let stmt = p.obj(&db, "query", vec![s(select), i(FETCH_NUM)]);
    p.m(
        &stmt,
        "setFetchMode",
        vec![i(FETCH_CLASS), s("TestClass"), arr(vec![s("Changed")])],
    );
    let r = p.m(&stmt, "fetchAll", vec![]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_012.phpt"));
}

/// `foreach ($stmt as $data) var_dump($data);` through getIterator().
fn foreach_dump(p: &mut P, stmt: &ObjRef) {
    let it = p.obj(stmt, "getIterator", vec![]);
    p.m(&it, "rewind", vec![]);
    while matches!(p.m(&it, "valid", vec![]), Value::Bool(true)) {
        let v = p.m(&it, "current", vec![]);
        p.dump(&v);
        p.m(&it, "next", vec![]);
    }
}

#[test]
fn pdo_013_iterator() {
    let mut p = P::new();
    let db = p.factory();
    group_table(&mut p, &db, "test013");
    let select = "SELECT val, grp FROM test013";
    let stmt = p.obj(&db, "prepare", vec![s(select)]);
    p.m(&stmt, "execute", vec![]);
    p.m(&stmt, "setFetchMode", vec![i(FETCH_NUM)]);
    foreach_dump(&mut p, &stmt);
    user_class(
        p.vm(),
        "TestClass",
        None,
        &[("val", PUBLIC), ("grp", PUBLIC)],
        Some((&ECHO_CTOR_NAME, PUBLIC)),
    );
    let stmt = p.obj(
        &db,
        "query",
        vec![s(select), i(FETCH_CLASS), s("TestClass")],
    );
    foreach_dump(&mut p, &stmt);
    let stmt = p.obj(
        &db,
        "query",
        vec![
            s(select),
            i(FETCH_CLASS),
            s("TestClass"),
            arr(vec![s("WOW")]),
        ],
    );
    foreach_dump(&mut p, &stmt);
    p.check(phpt!("pdo/tests/pdo_013.phpt"));
}

#[test]
fn pdo_015_fetch_column() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test015(id int NOT NULL PRIMARY KEY, val VARCHAR(10), val2 VARCHAR(20))",
    );
    p.exec(&db, "INSERT INTO test015 VALUES(1, 'A', 'A2')");
    p.exec(&db, "INSERT INTO test015 VALUES(2, 'A', 'B2')");
    let sel = p.obj(&db, "prepare", vec![s("SELECT id, val, val2 FROM test015")]);
    let cases: Vec<(&str, Vec<Value>)> = vec![
        ("PDO::FETCH_COLUMN", vec![i(FETCH_COLUMN)]),
        ("PDO::FETCH_COLUMN, 2", vec![i(FETCH_COLUMN), i(2)]),
        (
            "PDO::FETCH_COLUMN|PDO::FETCH_GROUP",
            vec![i(FETCH_COLUMN | FETCH_GROUP)],
        ),
        (
            "PDO::FETCH_COLUMN|PDO::FETCH_UNIQUE",
            vec![i(FETCH_COLUMN | FETCH_UNIQUE)],
        ),
        (
            "PDO::FETCH_COLUMN|PDO::FETCH_UNIQUE, 0",
            vec![i(FETCH_COLUMN | FETCH_UNIQUE), i(0)],
        ),
        (
            "PDO::FETCH_COLUMN|PDO::FETCH_UNIQUE, 1",
            vec![i(FETCH_COLUMN | FETCH_UNIQUE), i(1)],
        ),
        (
            "PDO::FETCH_COLUMN|PDO::FETCH_UNIQUE, 2",
            vec![i(FETCH_COLUMN | FETCH_UNIQUE), i(2)],
        ),
    ];
    for (label, args) in cases {
        p.m(&sel, "execute", vec![]);
        p.echo(format!("$selectIdVal1Val2->fetchAll({label})\n"));
        let r = p.m(&sel, "fetchAll", args);
        p.dump(&r);
    }
    let sel2 = p.obj(&db, "prepare", vec![s("SELECT val, val2 FROM test015")]);
    p.m(&sel2, "execute", vec![]);
    p.echo("$selectVal1Val2->fetchAll(PDO::FETCH_COLUMN|PDO::FETCH_GROUP)\n");
    let r = p.m(&sel2, "fetchAll", vec![i(FETCH_COLUMN | FETCH_GROUP)]);
    p.dump(&r);
    p.check(phpt!("pdo/tests/pdo_015.phpt"));
}

#[test]
fn pdo_016_fetch_bound() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test016(idx int NOT NULL PRIMARY KEY, txt VARCHAR(20))",
    );
    for n in 0..3 {
        p.exec(
            &db,
            &format!("INSERT INTO test016 VALUES({n}, 'String{n}')"),
        );
    }
    let stmt1 = p.obj(&db, "prepare", vec![s("SELECT COUNT(idx) FROM test016")]);
    let stmt2 = p.obj(
        &db,
        "prepare",
        vec![s("SELECT idx, txt FROM test016 ORDER by idx")],
    );
    p.m(&stmt1, "execute", vec![]);
    let r = p.m(&stmt1, "fetchColumn", vec![]);
    p.dump(&r);
    drop(stmt1);
    p.m(&stmt2, "execute", vec![]);
    let cont = p.m(&stmt2, "fetchAll", vec![i(FETCH_COLUMN | FETCH_UNIQUE)]);
    p.dump(&cont);

    p.echo("===WHILE===\n");
    let idx = php_runtime::PhpRef::new(Value::Null);
    let txt = php_runtime::PhpRef::new(Value::Null);
    p.m(
        &stmt2,
        "bindColumn",
        vec![s("idx"), Value::Ref(idx.clone())],
    );
    p.m(
        &stmt2,
        "bindColumn",
        vec![s("txt"), Value::Ref(txt.clone())],
    );
    p.m(&stmt2, "execute", vec![]);
    while matches!(
        p.m(&stmt2, "fetch", vec![i(FETCH_BOUND)]),
        Value::Bool(true)
    ) {
        let k = php_runtime::value::array::cast_key(&idx.get());
        let php_runtime::value::array::KeyCast::Key(k) = k else {
            panic!()
        };
        let mut a = php_runtime::PhpArray::new();
        a.insert(k, txt.get());
        p.dump(&Value::array(a));
    }

    p.echo("===ALONE===\n");
    let stmt3 = p.obj(
        &db,
        "prepare",
        vec![s("SELECT txt FROM test016 WHERE idx=:inp")],
    );
    p.m(
        &stmt3,
        "bindParam",
        vec![s(":inp"), Value::Ref(idx.clone())],
    );
    let stmt4 = p.obj(
        &db,
        "prepare",
        vec![s("SELECT idx FROM test016 WHERE txt=:txt")],
    );
    p.m(
        &stmt4,
        "bindParam",
        vec![s(":txt"), Value::Ref(txt.clone())],
    );
    let col1 = php_runtime::PhpRef::new(Value::Null);
    let col2 = php_runtime::PhpRef::new(Value::Null);
    let cont_arr = cont.as_array().cloned().unwrap();
    let pair = |p: &mut P, k: &Value, v: &Value| {
        let php_runtime::value::array::KeyCast::Key(k) = php_runtime::value::array::cast_key(k)
        else {
            panic!()
        };
        let mut a = php_runtime::PhpArray::new();
        a.insert(k, v.clone());
        p.dump(&Value::array(a));
    };
    for (k, v) in cont_arr.iter() {
        idx.set(k.to_value());
        txt.set(v.clone());
        pair(&mut p, &idx.get(), &txt.get());
        let r = p.m(&stmt3, "execute", vec![]);
        p.dump(&r);
        if k.as_int() == Some(0) {
            p.m(
                &stmt3,
                "bindColumn",
                vec![s("txt"), Value::Ref(col1.clone())],
            );
        }
        let r = p.m(&stmt3, "fetch", vec![i(FETCH_BOUND)]);
        p.dump(&r);
        p.m(&stmt3, "closeCursor", vec![]);
        let r = p.m(&stmt4, "execute", vec![]);
        p.dump(&r);
        if k.as_int() == Some(0) {
            p.m(
                &stmt4,
                "bindColumn",
                vec![s("idx"), Value::Ref(col2.clone())],
            );
        }
        let r = p.m(&stmt4, "fetch", vec![i(FETCH_BOUND)]);
        p.dump(&r);
        p.m(&stmt4, "closeCursor", vec![]);
        pair(&mut p, &col2.get(), &col1.get());
    }

    p.echo("===REBIND/SAME===\n");
    p.m(
        &stmt4,
        "bindColumn",
        vec![s("idx"), Value::Ref(col1.clone())],
    );
    for (k, v) in cont_arr.iter() {
        idx.set(k.to_value());
        txt.set(v.clone());
        pair(&mut p, &idx.get(), &txt.get());
        for st in [&stmt3, &stmt4] {
            let r = p.m(st, "execute", vec![]);
            p.dump(&r);
            let r = p.m(st, "fetch", vec![i(FETCH_BOUND)]);
            p.dump(&r);
            p.m(st, "closeCursor", vec![]);
            p.dump(&col1.get());
        }
    }

    p.echo("===REBIND/CONFLICT===\n");
    p.m(
        &stmt2,
        "bindColumn",
        vec![s("idx"), Value::Ref(col1.clone())],
    );
    p.m(
        &stmt2,
        "bindColumn",
        vec![s("txt"), Value::Ref(col1.clone())],
    );
    p.m(&stmt2, "execute", vec![]);
    while matches!(
        p.m(&stmt2, "fetch", vec![i(FETCH_BOUND)]),
        Value::Bool(true)
    ) {
        p.dump(&col1.get());
    }
    p.check(phpt!("pdo/tests/pdo_016.phpt"));
}

#[test]
fn pdo_017_transactions() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test017(id INT NOT NULL PRIMARY KEY, val VARCHAR(10))",
    );
    for (id, v) in [(1, "A"), (2, "B"), (3, "C")] {
        p.exec(&db, &format!("INSERT INTO test017 VALUES({id}, '{v}')"));
    }
    let delete = p.obj(&db, "prepare", vec![s("DELETE FROM test017")]);
    let count_rows = |p: &mut P, action: &str| {
        let select = p.obj(&db, "prepare", vec![s("SELECT COUNT(*) FROM test017")]);
        p.m(&select, "execute", vec![]);
        let res = p.m(&select, "fetchColumn", vec![]);
        let res = res.as_str().unwrap().to_string_lossy().into_owned();
        p.echo(format!("Counted {res} rows after {action}.\n"));
    };
    count_rows(&mut p, "insert");
    p.m(&db, "beginTransaction", vec![]);
    p.m(&delete, "execute", vec![]);
    count_rows(&mut p, "delete");
    p.m(&db, "rollBack", vec![]);
    count_rows(&mut p, "rollback");
    p.m(&db, "beginTransaction", vec![]);
    p.m(&delete, "execute", vec![]);
    count_rows(&mut p, "delete");
    p.m(&db, "commit", vec![]);
    count_rows(&mut p, "commit");
    p.check(phpt!("pdo/tests/pdo_017.phpt"));
}

#[test]
fn pdo_020_column_count() {
    let mut p = P::new();
    let db = p.factory();
    p.exec(
        &db,
        "CREATE TABLE test020(id INT NOT NULL PRIMARY KEY, val VARCHAR(10), val2 VARCHAR(16))",
    );
    for (id, v) in [(1, "A"), (2, "B"), (3, "C")] {
        p.exec(
            &db,
            &format!("INSERT INTO test020 VALUES({id}, '{v}', '{v}')"),
        );
    }
    for sql in [
        "SELECT id, val FROM test020",
        "SELECT id, val, val2 FROM test020",
        "SELECT COUNT(*) FROM test020",
    ] {
        let stmt = p.obj(&db, "query", vec![s(sql)]);
        let res = p.m(&stmt, "columnCount", vec![]).as_int().unwrap();
        p.echo(format!("Counted {res} columns after {sql}.\n"));
    }
    p.check(phpt!("pdo/tests/pdo_020.phpt"));
}
