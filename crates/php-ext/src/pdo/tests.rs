//! Unit tests ported from php-src `ext/pdo/tests` (run against
//! `sqlite::memory:` as with `PDOTEST_DSN=sqlite::memory:`) and
//! `ext/pdo_sqlite/tests`.

use php_runtime::{ObjRef, PResult, Value};

use crate::testing::{TestVm, thrown};

fn new_obj(t: &mut TestVm, class: &str, args: Vec<Value>) -> PResult<ObjRef> {
    let ce = t.vm.find_class(class).expect("class registered");
    let obj = t.vm.instantiate(&ce)?;
    t.vm.call_method(&obj, b"__construct", args)?;
    Ok(obj)
}

fn call(t: &mut TestVm, obj: &ObjRef, name: &str, args: Vec<Value>) -> Value {
    match t.vm.call_method(obj, name.as_bytes(), args) {
        Ok(v) => v,
        Err(e) => panic!("{}::{name}() threw {:?}", obj.class().name, thrown(Err(e))),
    }
}

#[test]
fn memory_database_round_trip() {
    let mut t = TestVm::new();
    let db = new_obj(&mut t, "PDO", vec![Value::from("sqlite::memory:")]).unwrap();
    assert_eq!(
        call(
            &mut t,
            &db,
            "exec",
            vec![Value::from("CREATE TABLE t(a INTEGER, b TEXT)")]
        )
        .as_int(),
        Some(0)
    );
    assert_eq!(
        call(
            &mut t,
            &db,
            "exec",
            vec![Value::from("INSERT INTO t VALUES (1, 'x')")]
        )
        .as_int(),
        Some(1)
    );
    let stmt = call(&mut t, &db, "query", vec![Value::from("SELECT * FROM t")]);
    let stmt = stmt.as_object().cloned().expect("statement");
    let row = call(&mut t, &stmt, "fetch", vec![Value::Int(2)]);
    let row = row.as_array().cloned().expect("row");
    assert_eq!(row.get_str(b"a").and_then(Value::as_int), Some(1));
}
