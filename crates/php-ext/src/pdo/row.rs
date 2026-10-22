//! `PDORow`: the lazily fetched row of `PDO::FETCH_LAZY` (`pdo_stmt.c`).
//!
//! Reads go to the statement's current row; every write or unset throws.
//! The runtime has no `has_property`/`unset_property`/`get_constructor`
//! hooks yet: `isset($row->col)` is exposed as [`prop_exists`] for the
//! interpreter, and `new PDORow` is refused by `create_object`.

use std::rc::Rc;

use php_runtime::class::{ClassEntry, DimFetch, PropPurpose, class_flags};
use php_runtime::value::numeric::{Numeric, is_numeric_str};
use php_runtime::{ArrayKey, ClassRef, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::error;
use super::stmt::{Stmt, declare_query_string, fetch_value};

pub const ROW_CLASS: &str = "PDORow";

/// Native state of a `PDORow` (`pdo_row_t`): the statement object it reads.
pub struct RowHandle {
    pub stmt_obj: ObjRef,
    pub stmt: Rc<Stmt>,
}

/// Creates the lazy row object of a statement.
pub fn new_row(_vm: &mut Vm, stmt_obj: &ObjRef, stmt: &Rc<Stmt>) -> PResult<ObjRef> {
    let ce = super::find_class(_vm, ROW_CLASS);
    let row = ObjRef::new(ce);
    row.set_native(RowHandle {
        stmt_obj: stmt_obj.clone(),
        stmt: stmt.clone(),
    });
    Ok(row)
}

fn handle(obj: &ObjRef) -> Option<(ObjRef, Rc<Stmt>)> {
    obj.native::<RowHandle>()
        .map(|h| (h.stmt_obj.clone(), h.stmt.clone()))
}

/// `row_read_column_name`.
fn read_column_name(vm: &mut Vm, stmt: &Stmt, name: &[u8]) -> PResult<Option<Value>> {
    let Some(dbh) = stmt.dbh() else {
        return Ok(None);
    };
    for colno in 0..stmt.column_count.get().max(0) as usize {
        if stmt
            .column_name(colno)
            .is_some_and(|n| n.as_bytes() == name)
        {
            return fetch_value(vm, stmt, &dbh, colno as i64, None).map(Some);
        }
    }
    Ok(None)
}

/// `row_read_column_number`.
fn read_column_number(vm: &mut Vm, stmt: &Stmt, column: i64) -> PResult<Option<Value>> {
    let Some(dbh) = stmt.dbh() else {
        return Ok(None);
    };
    if column >= 0 && column < stmt.column_count.get() as i64 {
        return fetch_value(vm, stmt, &dbh, column, None).map(Some);
    }
    Ok(None)
}

fn numeric_long(name: &[u8]) -> Option<i64> {
    match is_numeric_str(name) {
        Some(Numeric::Int(i)) => Some(i),
        _ => None,
    }
}

/// `row_prop_read`.
fn prop_read(vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<Option<Value>> {
    let Some((stmt_obj, stmt)) = handle(obj) else {
        return Ok(None);
    };
    if name.as_bytes() == b"queryString" {
        return Ok(Some(
            stmt_obj.get_prop(b"queryString").unwrap_or(Value::Null),
        ));
    }
    let v = match numeric_long(name) {
        Some(n) => read_column_number(vm, &stmt, n)?,
        None => read_column_name(vm, &stmt, name)?,
    };
    Ok(Some(v.unwrap_or(Value::Null)))
}

/// `row_dim_read`.
fn dim_read(vm: &mut Vm, obj: &ObjRef, offset: Option<&Value>, _fetch: DimFetch) -> PResult<Value> {
    let Some(offset) = offset else {
        return Err(vm.throw_error("Error", "Cannot append to PDORow offset"));
    };
    if let Value::Int(n) = offset.unref() {
        let Some((_, stmt)) = handle(obj) else {
            return Ok(Value::Null);
        };
        return Ok(read_column_number(vm, &stmt, n)?.unwrap_or(Value::Null));
    }
    let name = vm.to_php_string(&offset.unref())?;
    Ok(prop_read(vm, obj, &name)?.unwrap_or(Value::Null))
}

fn prop_write(vm: &mut Vm, _obj: &ObjRef, _name: &PhpStr, _value: Value) -> PResult<bool> {
    Err(vm.throw_error("Error", "Cannot write to PDORow property"))
}

fn dim_write(vm: &mut Vm, _obj: &ObjRef, offset: Option<&Value>, _value: Value) -> PResult<()> {
    match offset {
        None => Err(vm.throw_error("Error", "Cannot append to PDORow offset")),
        Some(_) => Err(vm.throw_error("Error", "Cannot write to PDORow offset")),
    }
}

/// `row_prop_exists` (`isset()`/`empty()` on properties).
pub fn prop_exists(vm: &mut Vm, obj: &ObjRef, name: &PhpStr, check_empty: bool) -> PResult<bool> {
    let Some((_, stmt)) = handle(obj) else {
        return Ok(false);
    };
    let v = match numeric_long(name) {
        Some(n) => read_column_number(vm, &stmt, n)?,
        None => read_column_name(vm, &stmt, name)?,
    };
    match v {
        None => Ok(false),
        Some(v) if check_empty => vm.to_bool(&v),
        Some(v) => Ok(!v.is_null()),
    }
}

/// `row_dim_exists`.
fn dim_exists(vm: &mut Vm, obj: &ObjRef, offset: &Value, check_empty: bool) -> PResult<bool> {
    if let Value::Int(column) = offset.unref() {
        let Some((_, stmt)) = handle(obj) else {
            return Ok(false);
        };
        if !check_empty {
            return Ok(column >= 0 && column < stmt.column_count.get() as i64);
        }
        return match read_column_number(vm, &stmt, column)? {
            Some(v) => vm.to_bool(&v),
            None => Ok(false),
        };
    }
    let name = vm.to_php_string(&offset.unref())?;
    prop_exists(vm, obj, &name, check_empty)
}

fn dim_unset(vm: &mut Vm, _obj: &ObjRef, _offset: &Value) -> PResult<()> {
    Err(vm.throw_error("Error", "Cannot unset PDORow offset"))
}

/// `row_prop_delete` (for the interpreter's `unset($row->x)`).
pub fn prop_unset(vm: &mut Vm) -> php_runtime::Flow {
    vm.throw_error("Error", "Cannot unset PDORow property")
}

/// `row_get_properties_for(ZEND_PROP_PURPOSE_DEBUG)`: the statement's
/// properties followed by the columns of the current row.
fn debug_info(vm: &mut Vm, obj: &ObjRef, purpose: PropPurpose) -> PResult<Option<PhpArray>> {
    if purpose != PropPurpose::Debug {
        return Ok(None);
    }
    let Some((stmt_obj, stmt)) = handle(obj) else {
        return Ok(None);
    };
    let mut props = PhpArray::new();
    for (name, value) in stmt_obj.properties() {
        props.insert(ArrayKey::Str(name), value);
    }
    if let Some(dbh) = stmt.dbh() {
        for i in 0..stmt.column_count.get().max(0) as usize {
            let Some(name) = stmt.column_name(i) else {
                continue;
            };
            if name.as_bytes() == b"queryString" {
                continue;
            }
            let v = fetch_value(vm, &stmt, &dbh, i as i64, None)?;
            props.insert(ArrayKey::Str(name), v);
        }
    }
    Ok(Some(props))
}

/// `row_get_ctor`: `new PDORow` is not allowed.
fn create_row(vm: &mut Vm, _ce: &ClassRef) -> PResult<ObjRef> {
    Err(error::throw(vm, "You may not create a PDORow manually"))
}

fn clone_forbidden(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!(
            "Trying to clone an uncloneable object of class {}",
            obj.class().name
        ),
    ))
}

/// The statement object a row reads from (tests and debugging).
pub fn row_statement(obj: &ObjRef) -> Option<ObjRef> {
    handle(obj).map(|(o, _)| o)
}

/// Registers `final class PDORow`.
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(
        ROW_CLASS,
        class_flags::FINAL | class_flags::NOT_SERIALIZABLE,
    );
    declare_query_string(&mut ce, ROW_CLASS);
    ce.handlers.create_object = Some(create_row);
    ce.handlers.clone_obj = Some(clone_forbidden);
    ce.handlers.read_property = Some(prop_read);
    ce.handlers.write_property = Some(prop_write);
    ce.handlers.read_dimension = Some(dim_read);
    ce.handlers.write_dimension = Some(dim_write);
    ce.handlers.has_dimension = Some(dim_exists);
    ce.handlers.unset_dimension = Some(dim_unset);
    ce.handlers.get_debug_info = Some(debug_info);
    r.class(ce);
}
