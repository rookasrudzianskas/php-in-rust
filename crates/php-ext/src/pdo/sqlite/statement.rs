//! `sqlite_statement.c`: the SQLite statement driver.

use std::any::Any;
use std::rc::Rc;

use php_runtime::{PResult, PhpArray, PhpStr, Value, Vm};

use crate::sqlite3::capi::{self, ValueRef, ffi};

use super::super::consts::{PARAM_BOOL, PARAM_INT, PARAM_LOB, PARAM_NULL, PARAM_STMT, PARAM_STR};
use super::super::driver::{AttrValue, DbhMethods, ParamEvent, StmtMethods};
use super::super::error::raise_impl_error;
use super::super::stmt::{BoundParam, ColumnData, Stmt, param_type_of};
use super::driver::SqliteHandle;
use super::{ATTR_BUSY_STATEMENT, ATTR_EXPLAIN_STATEMENT, ATTR_READONLY_STATEMENT};

/// `pdo_sqlite_stmt`.
pub struct SqliteStmt {
    h: Rc<dyn DbhMethods>,
    stmt: capi::Stmt,
    pre_fetched: bool,
    done: bool,
}

impl SqliteStmt {
    pub fn new(h: Rc<dyn DbhMethods>, stmt: capi::Stmt) -> SqliteStmt {
        SqliteStmt {
            h,
            stmt,
            pre_fetched: false,
            done: false,
        }
    }

    fn handle(&self) -> &SqliteHandle {
        self.h
            .as_any()
            .downcast_ref::<SqliteHandle>()
            .expect("SQLite statements belong to SQLite handles")
    }

    /// `pdo_sqlite_error_stmt`.
    fn error(&self, vm: &mut Vm, stmt: &Stmt) -> PResult<()> {
        if let Some(dbh) = stmt.dbh() {
            self.handle().error(vm, &dbh, Some(stmt))?;
        }
        Ok(())
    }

    /// Binds parameter `paramno` (0-based) for `PDO_PARAM_EVT_EXEC_PRE`.
    fn bind(&mut self, vm: &mut Vm, stmt: &Stmt, param: &mut BoundParam) -> PResult<bool> {
        let idx = (param.paramno + 1) as i32;
        let ty = param_type_of(param.param_type) as i64;
        let ok = |rc| rc == ffi::SQLITE_OK;
        match ty {
            PARAM_STMT => Ok(false),
            PARAM_NULL => {
                if ok(self.stmt.bind_null(idx)) {
                    return Ok(true);
                }
                self.error(vm, stmt)?;
                Ok(false)
            }
            PARAM_INT | PARAM_BOOL => {
                let v = param.value();
                if v.is_null() {
                    if ok(self.stmt.bind_null(idx)) {
                        return Ok(true);
                    }
                } else {
                    // convert_to_long() changes the bound variable itself.
                    let l = vm.to_int(&v)?;
                    param.set_value(Value::Int(l));
                    if ok(self.stmt.bind_int64(idx, l)) {
                        return Ok(true);
                    }
                }
                self.error(vm, stmt)?;
                Ok(false)
            }
            PARAM_LOB => {
                let v = param.value();
                let data = match &v {
                    Value::Resource(r) => {
                        if crate::standard::streams::handle_of(r).is_none() {
                            if let Some(dbh) = stmt.dbh() {
                                raise_impl_error(
                                    vm,
                                    &dbh,
                                    Some(stmt),
                                    "HY105",
                                    Some("Expected a stream resource"),
                                )?;
                            }
                            return Ok(false);
                        }
                        let mem = crate::standard::streams::read_to_end(vm, r)?.unwrap_or_default();
                        let s = PhpStr::from_vec(mem);
                        param.set_value(Value::String(s.clone()));
                        s
                    }
                    Value::Null => {
                        if ok(self.stmt.bind_null(idx)) {
                            return Ok(true);
                        }
                        self.error(vm, stmt)?;
                        return Ok(false);
                    }
                    Value::String(s) => s.clone(),
                    other => {
                        let s = vm.to_php_string(other)?;
                        param.set_value(Value::String(s.clone()));
                        s
                    }
                };
                Ok(ok(self.stmt.bind_blob(idx, &data)))
            }
            _ => {
                let v = param.value();
                if v.is_null() {
                    if ok(self.stmt.bind_null(idx)) {
                        return Ok(true);
                    }
                } else {
                    let s = match &v {
                        Value::String(s) => s.clone(),
                        other => {
                            let s = vm.to_php_string(other)?;
                            param.set_value(Value::String(s.clone()));
                            s
                        }
                    };
                    if ok(self.stmt.bind_text(idx, &s)) {
                        return Ok(true);
                    }
                }
                self.error(vm, stmt)?;
                Ok(false)
            }
        }
    }
}

impl StmtMethods for SqliteStmt {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// `pdo_sqlite_stmt_execute`.
    fn execute(&mut self, vm: &mut Vm, stmt: &Stmt) -> PResult<bool> {
        if stmt.executed.get() && !self.done {
            self.stmt.reset();
        }
        self.done = false;
        match self.stmt.step(vm) {
            ffi::SQLITE_ROW => {
                self.pre_fetched = true;
                stmt.set_column_count(self.stmt.data_count());
                Ok(true)
            }
            ffi::SQLITE_DONE => {
                stmt.set_column_count(self.stmt.column_count());
                stmt.row_count.set(self.handle().db.changes());
                self.stmt.reset();
                self.done = true;
                Ok(true)
            }
            rc => {
                if rc == ffi::SQLITE_ERROR {
                    self.stmt.reset();
                }
                self.error(vm, stmt)?;
                Ok(false)
            }
        }
    }

    /// `pdo_sqlite_stmt_fetch`.
    fn fetch(&mut self, vm: &mut Vm, stmt: &Stmt, _ori: i64, _offset: i64) -> PResult<bool> {
        if !self.stmt.is_live() {
            return Ok(false);
        }
        if self.pre_fetched {
            self.pre_fetched = false;
            return Ok(true);
        }
        if self.done {
            return Ok(false);
        }
        match self.stmt.step(vm) {
            ffi::SQLITE_ROW => Ok(true),
            ffi::SQLITE_DONE => {
                self.done = true;
                self.stmt.reset();
                Ok(false)
            }
            rc => {
                if rc == ffi::SQLITE_ERROR {
                    self.stmt.reset();
                }
                self.error(vm, stmt)?;
                Ok(false)
            }
        }
    }

    /// `pdo_sqlite_stmt_describe`.
    fn describe(&mut self, vm: &mut Vm, stmt: &Stmt, colno: i32) -> PResult<Option<ColumnData>> {
        if colno >= self.stmt.column_count() {
            self.error(vm, stmt)?;
            return Ok(None);
        }
        let name = self.stmt.column_name(colno).unwrap_or_default();
        Ok(Some(ColumnData {
            name: PhpStr::from_bytes(name),
            maxlen: -1,
            precision: 0,
        }))
    }

    /// `pdo_sqlite_stmt_get_col`.
    fn get_col(
        &mut self,
        vm: &mut Vm,
        stmt: &Stmt,
        colno: i32,
        _ty: Option<i32>,
    ) -> PResult<Value> {
        if !self.stmt.is_live() {
            return Ok(Value::Null);
        }
        if colno >= self.stmt.data_count() {
            self.error(vm, stmt)?;
            return Ok(Value::Null);
        }
        Ok(match self.stmt.column_value(colno) {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(i) => Value::Int(i),
            ValueRef::Real(d) => Value::Float(d),
            ValueRef::Text(s) | ValueRef::Blob(s) => Value::bytes(s),
        })
    }

    fn supports_param_hook(&self) -> bool {
        true
    }

    /// `pdo_sqlite_stmt_param_hook`.
    fn param_hook(
        &mut self,
        vm: &mut Vm,
        stmt: &Stmt,
        param: &mut BoundParam,
        event: ParamEvent,
    ) -> PResult<bool> {
        if event != ParamEvent::ExecPre {
            return Ok(true);
        }
        if stmt.executed.get() && !self.done {
            self.stmt.reset();
            self.done = true;
        }
        if !param.is_param {
            return Ok(true);
        }
        if param.paramno == -1 {
            let name = param.name.clone().unwrap_or_default();
            param.paramno = self.stmt.bind_parameter_index(&name) as i64 - 1;
        }
        self.bind(vm, stmt, param)
    }

    fn supports_set_attribute(&self) -> bool {
        true
    }

    /// `pdo_sqlite_stmt_set_attribute`.
    fn set_attribute(
        &mut self,
        vm: &mut Vm,
        _stmt: &Stmt,
        attr: i64,
        value: &Value,
    ) -> PResult<bool> {
        if attr != ATTR_EXPLAIN_STATEMENT {
            return Ok(false);
        }
        let Value::Int(mode) = value.unref() else {
            return Err(vm.throw_type_error(format!(
                "explain mode must be of type int, {} given",
                value.unref().value_name()
            )));
        };
        if !(0..=2).contains(&mode) {
            return Err(vm.throw_value_error(
                "explain mode must be one of the Pdo\\Sqlite::EXPLAIN_MODE_* constants",
            ));
        }
        Ok(self.stmt.explain_vm(vm, mode as i32) == ffi::SQLITE_OK)
    }

    fn supports_get_attribute(&self) -> bool {
        true
    }

    /// `pdo_sqlite_stmt_get_attribute`.
    fn get_attribute(&mut self, _vm: &mut Vm, _stmt: &Stmt, attr: i64) -> PResult<AttrValue> {
        Ok(match attr {
            ATTR_READONLY_STATEMENT => AttrValue::Value(Value::Bool(self.stmt.readonly())),
            ATTR_BUSY_STATEMENT => AttrValue::Value(Value::Bool(self.stmt.busy())),
            ATTR_EXPLAIN_STATEMENT => AttrValue::Value(Value::Int(self.stmt.isexplain() as i64)),
            _ => AttrValue::Unsupported,
        })
    }

    fn supports_column_meta(&self) -> bool {
        true
    }

    /// `pdo_sqlite_stmt_col_meta`.
    fn column_meta(&mut self, vm: &mut Vm, stmt: &Stmt, colno: i64) -> PResult<Option<PhpArray>> {
        if !self.stmt.is_live() || !stmt.executed.get() {
            return Ok(None);
        }
        if colno >= self.stmt.column_count() as i64 {
            self.error(vm, stmt)?;
            return Ok(None);
        }
        let col = colno as i32;
        let mut meta = PhpArray::new();
        let mut flags = PhpArray::new();
        let (native, pdo_type) = match self.stmt.column_type(col) {
            ffi::SQLITE_NULL => ("null", PARAM_NULL),
            ffi::SQLITE_FLOAT => ("double", PARAM_STR),
            ffi::SQLITE_BLOB => {
                let _ = flags.append(Value::from("blob"));
                ("string", PARAM_STR)
            }
            ffi::SQLITE_TEXT => ("string", PARAM_STR),
            _ => ("integer", PARAM_INT),
        };
        meta.insert_str("native_type", Value::from(native));
        meta.insert_str("pdo_type", Value::Int(pdo_type));
        if let Some(decl) = self.stmt.column_decltype(col) {
            meta.insert_str("sqlite:decl_type", Value::bytes(decl));
        }
        if let Some(table) = self.stmt.column_table_name(col) {
            meta.insert_str("table", Value::bytes(table));
        }
        meta.insert_str("flags", Value::array(flags));
        Ok(Some(meta))
    }

    fn supports_cursor_closer(&self) -> bool {
        true
    }

    /// `pdo_sqlite_stmt_cursor_closer`.
    fn close_cursor(&mut self, _vm: &mut Vm, _stmt: &Stmt) -> PResult<bool> {
        self.stmt.reset();
        Ok(true)
    }
}
