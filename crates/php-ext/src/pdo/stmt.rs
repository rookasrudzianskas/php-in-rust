//! `PDOStatement` (`pdo_stmt.c`): parameter binding, the fetch modes and
//! the statement methods.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, PropertyInfo, class_flags, member_flags};
use php_runtime::object::WeakObjRef;
use php_runtime::types::TypeDecl;
use php_runtime::value::convert::to_int_silent;
use php_runtime::{
    Args, ArrayKey, CallArgs, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray,
    PhpArrayRef, PhpStr, Registry, Value, Vm,
};

use crate::standard::streams::memory;

use super::dbh::{CASE_LOWER, CASE_NATURAL, CASE_UPPER, Dbh, NULL_EMPTY_STRING, NULL_TO_STRING};
use super::driver::{AttrValue, ParamEvent, StmtMethods};
use super::error::{self, ERR_NONE, ErrCode, err_str, handle_stmt_err, raise_impl_error};
use super::{consts::*, finish, iterator, parser, props, row};

pub const STMT_CLASS: &str = "PDOStatement";

/// `enum pdo_placeholder_support`.
pub const PLACEHOLDER_NONE: u8 = 0;
pub const PLACEHOLDER_NAMED: u8 = 1;
pub const PLACEHOLDER_POSITIONAL: u8 = 2;

/// `struct pdo_column_data`.
#[derive(Clone, Debug, Default)]
pub struct ColumnData {
    pub name: PhpStr,
    pub maxlen: i64,
    pub precision: u64,
}

/// Key of a bound parameter or column (`zend_hash` string or index key).
#[derive(Clone, Debug, PartialEq)]
pub enum ParamKey {
    Int(i64),
    Str(PhpStr),
}

/// `struct pdo_bound_param_data`.
#[derive(Clone, Debug)]
pub struct BoundParam {
    /// The bound variable (a `Value::Ref` for `bindParam()`/`bindColumn()`).
    pub parameter: Value,
    /// Optional driver parameters (`Undef` when not given).
    pub driver_params: Value,
    /// Zero-based position; `-1` for a name whose position is unknown yet.
    pub paramno: i64,
    pub name: Option<PhpStr>,
    pub max_value_len: i64,
    /// `enum pdo_param_type` with flags, as a C `int`.
    pub param_type: i32,
    pub is_param: bool,
}

impl BoundParam {
    fn new(parameter: Value, param_type: i32) -> BoundParam {
        BoundParam {
            parameter,
            driver_params: Value::Undef,
            paramno: -1,
            name: None,
            max_value_len: 0,
            param_type,
            is_param: true,
        }
    }

    /// The bound value (dereferenced).
    pub fn value(&self) -> Value {
        self.parameter.unref()
    }

    /// Replaces the bound value (through the reference, if any).
    pub fn set_value(&mut self, v: Value) {
        match &self.parameter {
            Value::Ref(r) => {
                r.set(v);
            }
            _ => self.parameter = v,
        }
    }
}

/// `PDO_PARAM_TYPE(x)`.
pub fn param_type_of(t: i32) -> i32 {
    (t as u32 & !PARAM_FLAGS) as i32
}

/// An ordered parameter table (`HashTable` of `pdo_bound_param_data`).
#[derive(Clone, Debug, Default)]
pub struct ParamTable {
    entries: Vec<(ParamKey, BoundParam)>,
}

impl ParamTable {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &ParamKey) -> Option<&BoundParam> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, p)| p)
    }

    pub fn get_mut(&mut self, key: &ParamKey) -> Option<&mut BoundParam> {
        self.entries
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, p)| p)
    }

    /// `zend_hash_update`: an existing key keeps its position.
    pub fn update(&mut self, key: ParamKey, p: BoundParam) {
        match self.get_mut(&key) {
            Some(slot) => *slot = p,
            None => self.entries.push((key, p)),
        }
    }

    pub fn remove(&mut self, key: &ParamKey) -> Option<BoundParam> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(i).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = &(ParamKey, BoundParam)> {
        self.entries.iter()
    }

    pub fn keys(&self) -> Vec<ParamKey> {
        self.entries.iter().map(|(k, _)| k.clone()).collect()
    }
}

/// The `stmt->fetch` union (kept as separate fields).
#[derive(Clone, Default)]
pub struct FetchState {
    pub column: i64,
    pub cls_ce: Option<ClassRef>,
    pub cls_ctor_args: Option<PhpArrayRef>,
    pub func: Option<Value>,
    pub into: Option<ObjRef>,
}

/// `stmt->active_query_string` relative to `query_string`.
#[derive(Clone, Default)]
pub enum ActiveQuery {
    #[default]
    None,
    /// The same string as the query (`PDO::query()`, unchanged emulation).
    SameAsQuery,
    /// The query with emulated binds expanded.
    Rewritten(PhpStr),
}

/// `pdo_stmt_t`.
pub struct Stmt {
    pub driver: RefCell<Option<Box<dyn StmtMethods>>>,
    pub error_code: Cell<ErrCode>,
    pub executed: Cell<bool>,
    pub in_fetch: Cell<bool>,
    pub supports_placeholders: Cell<u8>,
    pub in_param_event: Cell<bool>,
    pub execute_incomplete: Cell<bool>,
    pub bound_params: RefCell<Option<ParamTable>>,
    /// Rewriting map: positions and names to the rewritten names.
    pub bound_param_map: RefCell<Option<Vec<(ParamKey, PhpStr)>>>,
    pub bound_columns: RefCell<Option<ParamTable>>,
    pub columns: RefCell<Option<Vec<ColumnData>>>,
    pub column_count: Cell<i32>,
    pub default_fetch_type: Cell<i64>,
    pub fetch: RefCell<FetchState>,
    pub lazy_object_ref: RefCell<Option<WeakObjRef>>,
    pub dbh: RefCell<Option<Rc<Dbh>>>,
    pub database_object_handle: RefCell<Option<ObjRef>>,
    pub row_count: Cell<i64>,
    pub query_string: RefCell<Option<PhpStr>>,
    pub active_query_string: RefCell<ActiveQuery>,
    pub named_rewrite_template: Cell<Option<&'static str>>,
}

impl Default for Stmt {
    fn default() -> Self {
        Stmt::new()
    }
}

impl Stmt {
    pub fn new() -> Stmt {
        Stmt {
            driver: RefCell::new(None),
            error_code: Cell::new([0; 5]),
            executed: Cell::new(false),
            in_fetch: Cell::new(false),
            supports_placeholders: Cell::new(PLACEHOLDER_NONE),
            in_param_event: Cell::new(false),
            execute_incomplete: Cell::new(false),
            bound_params: RefCell::new(None),
            bound_param_map: RefCell::new(None),
            bound_columns: RefCell::new(None),
            columns: RefCell::new(None),
            column_count: Cell::new(0),
            default_fetch_type: Cell::new(0),
            fetch: RefCell::new(FetchState::default()),
            lazy_object_ref: RefCell::new(None),
            dbh: RefCell::new(None),
            database_object_handle: RefCell::new(None),
            row_count: Cell::new(0),
            query_string: RefCell::new(None),
            active_query_string: RefCell::new(ActiveQuery::None),
            named_rewrite_template: Cell::new(None),
        }
    }

    pub fn dbh(&self) -> Option<Rc<Dbh>> {
        self.dbh.borrow().clone()
    }

    /// `PDO_STMT_CLEAR_ERR()`.
    pub fn clear_err(&self) {
        self.error_code.set(ERR_NONE);
    }

    pub fn query_string(&self) -> PhpStr {
        self.query_string.borrow().clone().unwrap_or_default()
    }

    /// `php_pdo_stmt_set_column_count`.
    pub fn set_column_count(&self, new_count: i32) {
        if self.columns.borrow().is_none() {
            self.column_count.set(new_count);
            return;
        }
        if new_count == self.column_count.get() {
            return;
        }
        self.reset_columns();
        self.column_count.set(new_count);
    }

    /// `pdo_stmt_reset_columns`.
    pub fn reset_columns(&self) {
        *self.columns.borrow_mut() = None;
        self.column_count.set(0);
    }

    pub fn column_name(&self, i: usize) -> Option<PhpStr> {
        self.columns
            .borrow()
            .as_ref()
            .and_then(|c| c.get(i).map(|c| c.name.clone()))
    }

    /// Runs `f` on the driver statement. Re-entering a statement whose
    /// driver is running (from a user callback) is refused.
    pub fn with_driver<R>(
        &self,
        vm: &mut Vm,
        f: impl FnOnce(&mut dyn StmtMethods, &mut Vm) -> PResult<R>,
    ) -> PResult<R> {
        let Ok(mut guard) = self.driver.try_borrow_mut() else {
            return Err(vm.throw_error(
                "Error",
                "Cannot use a PDOStatement while its driver is running",
            ));
        };
        match guard.as_mut() {
            Some(d) => f(d.as_mut(), vm),
            None => Err(vm.throw_error("Error", "PDOStatement has no driver statement")),
        }
    }

    fn driver_supports(&self, f: impl Fn(&dyn StmtMethods) -> bool) -> bool {
        match self.driver.try_borrow() {
            Ok(d) => d.as_deref().is_some_and(f),
            Err(_) => false,
        }
    }
}

/// Native state of `PDOStatement` objects.
pub struct StmtHandle(pub Rc<Stmt>);

/// The statement of a `PDOStatement` object.
pub fn stmt_of(obj: &ObjRef) -> Option<Rc<Stmt>> {
    obj.native::<StmtHandle>().map(|h| h.0.clone())
}

/// `PHP_STMT_GET_OBJ`: the statement and its handle, or the
/// "object is uninitialized" Error.
pub fn get_obj(vm: &mut Vm, args: &Args) -> PResult<(ObjRef, Rc<Stmt>, Rc<Dbh>)> {
    let this = args
        .this()
        .cloned()
        .expect("PDOStatement method without $this");
    let stmt = stmt_of(&this);
    match stmt.as_ref().and_then(|s| s.dbh()) {
        Some(dbh) => Ok((this, stmt.expect("checked"), dbh)),
        None => Err(vm.throw_error(
            "Error",
            format!("{} object is uninitialized", this.class().name),
        )),
    }
}

// ---------------------------------------------------------------------------
// Columns and parameters
// ---------------------------------------------------------------------------

/// `pdo_stmt_describe_columns`.
pub fn describe_columns(vm: &mut Vm, stmt: &Stmt, dbh: &Dbh) -> PResult<bool> {
    let count = stmt.column_count.get().max(0);
    *stmt.columns.borrow_mut() = Some(vec![ColumnData::default(); count as usize]);
    for col in 0..count {
        let Some(mut data) = stmt.with_driver(vm, |d, vm| d.describe(vm, stmt, col))? else {
            return Ok(false);
        };
        let desired = dbh.desired_case.get();
        if dbh.native_case.get() != desired && desired != CASE_NATURAL {
            data.name = match desired {
                CASE_LOWER => data.name.to_ascii_lowercase(),
                CASE_UPPER => data.name.to_ascii_uppercase(),
                _ => data.name,
            };
        }
        if let Some(bc) = stmt.bound_columns.borrow_mut().as_mut() {
            if let Some(p) = bc.get_mut(&ParamKey::Str(data.name.clone())) {
                p.paramno = col as i64;
            }
        }
        if let Some(cols) = stmt.columns.borrow_mut().as_mut() {
            if let Some(slot) = cols.get_mut(col as usize) {
                *slot = data;
            }
        }
    }
    Ok(true)
}

/// `pdo_stmt_disallow_reentrant_param_event`.
fn disallow_reentrant_param_event(vm: &mut Vm, stmt: &Stmt) -> PResult<()> {
    if stmt.in_param_event.get() {
        return Err(vm.throw_error(
            "Error",
            "Cannot modify a PDOStatement while parameter hooks are running",
        ));
    }
    Ok(())
}

/// `dispatch_param_event`: runs the driver hook on every bound parameter,
/// then on every bound column.
pub fn dispatch_param_event(
    vm: &mut Vm,
    stmt: &Stmt,
    dbh: &Dbh,
    event: ParamEvent,
) -> PResult<bool> {
    if dbh.skip_param_evt.get() & (1 << event as u8) != 0 {
        return Ok(true);
    }
    if !stmt.driver_supports(|d| d.supports_param_hook()) {
        return Ok(true);
    }
    stmt.in_param_event.set(true);
    let r = (|| {
        for table in [&stmt.bound_params, &stmt.bound_columns] {
            let keys = table.borrow().as_ref().map(ParamTable::keys);
            for key in keys.unwrap_or_default() {
                let Some(mut p) = table.borrow().as_ref().and_then(|t| t.get(&key).cloned()) else {
                    continue;
                };
                let ok = stmt.with_driver(vm, |d, vm| d.param_hook(vm, stmt, &mut p, event))?;
                if let Some(slot) = table.borrow_mut().as_mut().and_then(|t| t.get_mut(&key)) {
                    *slot = p;
                }
                if !ok {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    })();
    stmt.in_param_event.set(false);
    r
}

/// `rewrite_name_to_position`.
fn rewrite_name_to_position(
    vm: &mut Vm,
    stmt: &Stmt,
    dbh: &Dbh,
    param: &mut BoundParam,
) -> PResult<bool> {
    let map = stmt.bound_param_map.borrow().clone();
    let Some(map) = map else {
        return Ok(true);
    };
    if stmt.named_rewrite_template.get().is_some() {
        return Ok(true);
    }
    let Some(name) = param.name.clone() else {
        if let Some((_, n)) = map.iter().find(|(k, _)| *k == ParamKey::Int(param.paramno)) {
            param.name = Some(n.clone());
            return Ok(true);
        }
        raise_impl_error(
            vm,
            dbh,
            Some(stmt),
            "HY093",
            Some("parameter was not defined"),
        )?;
        return Ok(false);
    };
    for (position, (_, n)) in map.iter().enumerate() {
        if *n != name {
            continue;
        }
        if param.paramno >= 0 {
            raise_impl_error(
                vm,
                dbh,
                Some(stmt),
                "IM001",
                Some(
                    "PDO refuses to handle repeating the same :named parameter for multiple positions with this driver, as it might be unsafe to do so.  Consider using a separate name for each parameter instead",
                ),
            )?;
            return Ok(false);
        }
        param.paramno = position as i64;
        return Ok(true);
    }
    raise_impl_error(
        vm,
        dbh,
        Some(stmt),
        "HY093",
        Some("parameter was not defined"),
    )?;
    Ok(false)
}

/// `really_register_bound_param`.
fn register_bound_param(
    vm: &mut Vm,
    stmt: &Stmt,
    dbh: &Dbh,
    mut param: BoundParam,
    is_param: bool,
) -> PResult<bool> {
    let table = if is_param {
        &stmt.bound_params
    } else {
        &stmt.bound_columns
    };
    table.borrow_mut().get_or_insert_with(ParamTable::default);

    let ty = param_type_of(param.param_type);
    let cur = param.value();
    if ty == PARAM_STR as i32 && param.max_value_len <= 0 && !cur.is_null() {
        let s = vm.to_php_string(&cur)?;
        if !matches!(cur, Value::String(_)) {
            param.set_value(Value::String(s));
        }
    } else if ty == PARAM_INT as i32 && matches!(cur, Value::Bool(_)) {
        param.set_value(Value::Int(to_int_silent(&cur)));
    } else if ty == PARAM_BOOL as i32 && matches!(cur, Value::Int(_)) {
        param.set_value(Value::Bool(to_int_silent(&cur) != 0));
    }
    param.is_param = is_param;

    if !is_param {
        if let Some(name) = param.name.clone() {
            let found = stmt
                .columns
                .borrow()
                .as_ref()
                .map(|cols| cols.iter().position(|c| c.name == name));
            if let Some(found) = found {
                if let Some(i) = found {
                    param.paramno = i as i64;
                }
                if param.paramno == -1 {
                    return Err(vm.argument_value_error(
                        1,
                        &format!(
                            "must refer to a column present in the result set, \"{name}\" given"
                        ),
                    ));
                }
            }
        }
    }

    if let Some(name) = &param.name {
        if is_param && name.as_bytes().first() != Some(&b':') {
            let mut n = Vec::with_capacity(name.len() + 1);
            n.push(b':');
            n.extend_from_slice(name);
            param.name = Some(PhpStr::from_vec(n));
        }
    }

    if is_param && !rewrite_name_to_position(vm, stmt, dbh, &mut param)? {
        return Ok(false);
    }

    if stmt.driver_supports(|d| d.supports_param_hook()) {
        let ok = stmt.with_driver(vm, |d, vm| {
            d.param_hook(vm, stmt, &mut param, ParamEvent::Normalize)
        })?;
        if !ok {
            handle_stmt_err(vm, dbh, stmt)?;
            return Ok(false);
        }
    }

    let key = match &param.name {
        Some(n) => ParamKey::Str(n.clone()),
        None => ParamKey::Int(param.paramno),
    };
    {
        let mut t = table.borrow_mut();
        let t = t.get_or_insert_with(ParamTable::default);
        if param.paramno >= 0 {
            t.remove(&ParamKey::Int(param.paramno));
        }
        t.update(key.clone(), param);
    }

    if stmt.driver_supports(|d| d.supports_param_hook()) {
        let Some(mut stored) = table.borrow().as_ref().and_then(|t| t.get(&key).cloned()) else {
            return Ok(true);
        };
        let ok = stmt.with_driver(vm, |d, vm| {
            d.param_hook(vm, stmt, &mut stored, ParamEvent::Alloc)
        })?;
        if !ok {
            handle_stmt_err(vm, dbh, stmt)?;
            if let Some(t) = table.borrow_mut().as_mut() {
                t.remove(&key);
            }
            return Ok(false);
        }
        if let Some(slot) = table.borrow_mut().as_mut().and_then(|t| t.get_mut(&key)) {
            *slot = stored;
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// `fetch_value`: column `colno` of the current row with PDO's
/// conversions (`ATTR_ORACLE_NULLS`, `ATTR_STRINGIFY_FETCHES`, bound types).
pub fn fetch_value(
    vm: &mut Vm,
    stmt: &Stmt,
    dbh: &Dbh,
    colno: i64,
    type_override: Option<i32>,
) -> PResult<Value> {
    if colno < 0 {
        return Err(vm.throw_value_error("Column index must be greater than or equal to 0"));
    }
    if colno >= stmt.column_count.get() as i64 {
        return Err(vm.throw_value_error("Invalid column index"));
    }
    let mut dest =
        stmt.with_driver(vm, |d, vm| d.get_col(vm, stmt, colno as i32, type_override))?;
    if matches!(&dest, Value::String(s) if s.is_empty())
        && dbh.oracle_nulls.get() == NULL_EMPTY_STRING
    {
        dest = Value::Null;
    }
    let type_override = if dbh.stringify.get() {
        Some(PARAM_STR as i32)
    } else {
        type_override
    };
    if let Some(t) = type_override {
        if !dest.is_null() {
            dest = match t {
                t if t == PARAM_INT as i32 => Value::Int(vm.to_int(&dest)?),
                t if t == PARAM_BOOL as i32 => Value::Bool(vm.to_bool(&dest)?),
                t if t == PARAM_STR as i32 => match &dest {
                    // "0" rather than "": what drivers without a boolean
                    // type would return.
                    Value::Bool(false) => Value::from("0"),
                    Value::Resource(r) => {
                        let r = r.clone();
                        let data = crate::standard::streams::read_to_end(vm, &r)?;
                        Value::from(data.unwrap_or_default())
                    }
                    other => Value::String(vm.to_php_string(other)?),
                },
                t if t == PARAM_NULL as i32 => Value::Null,
                t if t == PARAM_LOB as i32 => match &dest {
                    Value::String(s) => {
                        let s = s.clone();
                        Value::Resource(memory::memory_open(
                            vm,
                            memory::TEMP_STREAM_READONLY,
                            s.to_vec(),
                        ))
                    }
                    _ => dest,
                },
                _ => dest,
            };
        }
    }
    if dest.is_null() && dbh.oracle_nulls.get() == NULL_TO_STRING {
        dest = Value::empty_string();
    }
    Ok(dest)
}

/// `do_fetch_common`: advances the cursor and updates bound columns.
fn do_fetch_common(vm: &mut Vm, stmt: &Stmt, dbh: &Dbh, ori: i64, offset: i64) -> PResult<bool> {
    if !stmt.executed.get() || stmt.execute_incomplete.get() {
        return Ok(false);
    }
    if !dispatch_param_event(vm, stmt, dbh, ParamEvent::FetchPre)? {
        return Ok(false);
    }
    if !stmt.with_driver(vm, |d, vm| d.fetch(vm, stmt, ori, offset))? {
        return Ok(false);
    }
    if stmt.columns.borrow().is_none() && !describe_columns(vm, stmt, dbh)? {
        return Ok(false);
    }
    if !dispatch_param_event(vm, stmt, dbh, ParamEvent::FetchPost)? {
        return Ok(false);
    }
    let bound: Vec<(i64, Value, i32)> = stmt
        .bound_columns
        .borrow()
        .as_ref()
        .map(|t| {
            t.iter()
                .filter(|(_, p)| p.paramno >= 0 && p.parameter.is_ref())
                .map(|(_, p)| (p.paramno, p.parameter.clone(), p.param_type))
                .collect()
        })
        .unwrap_or_default();
    for (colno, var, ty) in bound {
        let v = fetch_value(vm, stmt, dbh, colno, Some(ty))?;
        if let Value::Ref(r) = var {
            r.set(v);
        }
    }
    Ok(true)
}

/// `pdo_do_key_pair_fetch`.
fn key_pair_fetch(
    vm: &mut Vm,
    stmt: &Stmt,
    dbh: &Dbh,
    ori: i64,
    offset: i64,
    container: &mut PhpArray,
) -> PResult<bool> {
    if !do_fetch_common(vm, stmt, dbh, ori, offset)? {
        return Ok(false);
    }
    if stmt.column_count.get() != 2 {
        raise_impl_error(
            vm,
            dbh,
            Some(stmt),
            "HY000",
            Some(
                "PDO::FETCH_KEY_PAIR fetch mode requires the result set to contain exactly 2 columns.",
            ),
        )?;
        return Ok(false);
    }
    let key = fetch_value(vm, stmt, dbh, 0, None)?;
    let val = fetch_value(vm, stmt, dbh, 1, None)?;
    match key {
        Value::Int(i) => {
            container.insert(ArrayKey::Int(i), val);
        }
        other => {
            let s = vm.to_php_string(&other)?;
            container.insert(ArrayKey::from_bytes(&s), val);
        }
    }
    Ok(true)
}

/// Splits constructor arguments into positional and named ones, as
/// `zend_call_function()` does with `named_params`.
type CtorArgs = (Vec<Value>, Vec<(PhpStr, Value)>);

fn split_ctor_args(vm: &mut Vm, args: Option<&PhpArrayRef>) -> PResult<CtorArgs> {
    let mut positional = Vec::new();
    let mut named = Vec::new();
    if let Some(args) = args {
        for (k, v) in args.iter() {
            match k {
                ArrayKey::Str(s) => named.push((s.clone(), v.clone())),
                ArrayKey::Int(_) => {
                    if !named.is_empty() {
                        return Err(vm.throw_error(
                            "Error",
                            "Cannot use positional argument after named argument",
                        ));
                    }
                    positional.push(v.clone());
                }
            }
        }
    }
    Ok((positional, named))
}

/// `pdo_call_fetch_object_constructor` (`zend_call_known_function` with the
/// constructor arguments as named parameters).
pub fn call_constructor(vm: &mut Vm, obj: &ObjRef, ctor_args: Option<&PhpArrayRef>) -> PResult<()> {
    let class = obj.class().clone();
    let Some(ctor) = class.magic.constructor.clone() else {
        return Ok(());
    };
    let (positional, named) = split_ctor_args(vm, ctor_args)?;
    vm.call_function(
        &ctor,
        CallArgs {
            positional,
            named,
            this: Some(obj.clone()),
            scope: Some(class),
            strict: false,
            dynamic: true,
        },
    )?;
    Ok(())
}

/// `object_init_ex`: refuses abstract classes, interfaces, traits and enums.
pub fn object_init_ex(vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let kind = if ce.is_interface() {
        Some("interface")
    } else if ce.is_trait() {
        Some("trait")
    } else if ce.is_enum() {
        Some("enum")
    } else if ce.is_abstract() {
        Some("abstract class")
    } else {
        None
    };
    if let Some(kind) = kind {
        return Err(vm.throw_error("Error", format!("Cannot instantiate {kind} {}", ce.name)));
    }
    vm.instantiate(ce)
}

/// The `PDORow` of a statement (`pdo_get_lazy_object`).
fn lazy_object(vm: &mut Vm, this: &ObjRef, stmt: &Rc<Stmt>) -> PResult<Value> {
    if let Some(row) = stmt
        .lazy_object_ref
        .borrow()
        .as_ref()
        .and_then(WeakObjRef::upgrade)
    {
        return Ok(Value::Object(row));
    }
    let row = row::new_row(vm, this, stmt)?;
    *stmt.lazy_object_ref.borrow_mut() = Some(row.downgrade());
    Ok(Value::Object(row))
}

/// `do_fetch`: fetches the next row in mode `how`; `None` is `false`
/// (no row or an error that was reported).
#[allow(clippy::too_many_arguments)]
pub fn do_fetch(
    vm: &mut Vm,
    this: &ObjRef,
    stmt: &Rc<Stmt>,
    dbh: &Dbh,
    how: i64,
    ori: i64,
    offset: i64,
    group_key: Option<&mut Value>,
) -> PResult<Option<Value>> {
    let how = if how == FETCH_USE_DEFAULT {
        stmt.default_fetch_type.get()
    } else {
        how
    };
    let flags = how & FETCH_FLAGS;
    let how = how & !FETCH_FLAGS;

    if !do_fetch_common(vm, stmt, dbh, ori, offset)? {
        return Ok(None);
    }
    if how == FETCH_BOUND {
        return Ok(Some(Value::Bool(true)));
    }
    if how == FETCH_LAZY {
        return lazy_object(vm, this, stmt).map(Some);
    }
    if how == FETCH_COLUMN {
        let column = stmt.fetch.borrow().column;
        let mut colno = column;
        if flags & (FETCH_GROUP | FETCH_UNIQUE) != 0 && column == -1 {
            colno = 1;
        }
        if colno < 0 {
            return Err(vm.throw_value_error("Column index must be greater than or equal to 0"));
        }
        if colno >= stmt.column_count.get() as i64 {
            return Err(vm.throw_value_error("Invalid column index"));
        }
        let value = if flags == FETCH_GROUP && column == -1 {
            fetch_value(vm, stmt, dbh, 1, None)?
        } else if flags == FETCH_GROUP && colno != 0 {
            fetch_value(vm, stmt, dbh, 0, None)?
        } else {
            fetch_value(vm, stmt, dbh, colno, None)?
        };
        if let Some(gk) = group_key {
            let k = if flags == FETCH_GROUP && column > 0 {
                fetch_value(vm, stmt, dbh, colno, None)?
            } else {
                fetch_value(vm, stmt, dbh, 0, None)?
            };
            *gk = Value::String(vm.to_php_string(&k)?);
        }
        return Ok(Some(value));
    }

    stmt.in_fetch.set(true);
    let r = do_fetch_rows(vm, stmt, dbh, how, flags, group_key);
    stmt.in_fetch.set(false);
    r
}

fn do_fetch_rows(
    vm: &mut Vm,
    stmt: &Rc<Stmt>,
    dbh: &Dbh,
    how: i64,
    flags: i64,
    group_key: Option<&mut Value>,
) -> PResult<Option<Value>> {
    let column_count = stmt.column_count.get() as i64;
    let mut column_index = 0i64;
    let mut scope: Option<ClassRef> = None;
    let mut ctor_args: Option<PhpArrayRef> = None;
    let mut array = PhpArray::new();
    let mut object: Option<ObjRef> = None;
    let mut func_params: Vec<Value> = Vec::new();
    let mut func: Option<Value> = None;

    match how {
        FETCH_USE_DEFAULT | FETCH_ASSOC | FETCH_BOTH | FETCH_NUM | FETCH_NAMED => {}
        FETCH_OBJ => {
            let ce = super::find_class(vm, "stdClass");
            object = Some(vm.instantiate(&ce)?);
            scope = Some(ce);
        }
        FETCH_CLASS => {
            let mut ce = stmt.fetch.borrow().cls_ce.clone();
            if flags & FETCH_CLASSTYPE != 0 {
                let name = fetch_value(vm, stmt, dbh, column_index, None)?;
                column_index += 1;
                // Only a string column replaces the fetch class.
                if let Value::String(s) = &name {
                    ce = vm.lookup_class(s)?;
                }
                if ce.is_none() {
                    ce = Some(super::find_class(vm, "stdClass"));
                }
            } else if ce.is_none() {
                raise_impl_error(
                    vm,
                    dbh,
                    Some(stmt),
                    "HY000",
                    Some("No fetch class specified"),
                )?;
                return Ok(None);
            }
            let ce = ce.expect("set above");
            ctor_args = stmt.fetch.borrow().cls_ctor_args.clone();
            if flags & FETCH_SERIALIZE != 0 {
                if !ce.instance_of_name(b"Serializable") {
                    // As this option is deprecated the class name is not mentioned.
                    raise_impl_error(
                        vm,
                        dbh,
                        Some(stmt),
                        "HY000",
                        Some("cannot unserialize class"),
                    )?;
                    return Ok(None);
                }
            } else {
                let obj = object_init_ex(vm, &ce)?;
                if ce.magic.constructor.is_some() && flags & FETCH_PROPS_LATE != 0 {
                    call_constructor(vm, &obj, ctor_args.as_ref())?;
                }
                object = Some(obj);
            }
            scope = Some(ce);
        }
        FETCH_INTO => {
            let into = stmt.fetch.borrow().into.clone();
            let Some(into) = into else {
                raise_impl_error(
                    vm,
                    dbh,
                    Some(stmt),
                    "HY000",
                    Some("No fetch-into object specified."),
                )?;
                return Ok(None);
            };
            // Fetching into an object behaves as if called from the global scope.
            object = Some(into);
        }
        FETCH_FUNC => {
            func = stmt.fetch.borrow().func.clone();
            if func.is_none() {
                raise_impl_error(
                    vm,
                    dbh,
                    Some(stmt),
                    "HY000",
                    Some("No fetch function specified"),
                )?;
                return Ok(None);
            }
        }
        _ => {}
    }

    if let Some(gk) = group_key {
        let k = fetch_value(vm, stmt, dbh, column_index, None)?;
        *gk = Value::String(vm.to_php_string(&k)?);
        column_index += 1;
    }

    if how == FETCH_CLASS && flags & FETCH_SERIALIZE != 0 {
        let data = fetch_value(vm, stmt, dbh, column_index, None)?;
        let data = match data {
            Value::String(s) => s,
            _ => PhpStr::new(),
        };
        let ce = scope.clone().expect("class fetch has a class");
        // `zend_user_unserialize`: a fresh instance whose unserialize() is called.
        let obj = vm.instantiate(&ce)?;
        vm.call_method(&obj, b"unserialize", vec![Value::String(data)])?;
        object = Some(obj);
        column_index += 1;
    }

    while column_index < column_count {
        let val = fetch_value(vm, stmt, dbh, column_index, None)?;
        let name = stmt.column_name(column_index as usize).unwrap_or_default();
        match how {
            FETCH_ASSOC => {
                array.insert(ArrayKey::from_bytes(&name), val);
            }
            FETCH_USE_DEFAULT | FETCH_BOTH => {
                array.insert(ArrayKey::from_bytes(&name), val.clone());
                if !array.contains_int(column_index) {
                    array.insert(ArrayKey::Int(column_index), val);
                }
            }
            FETCH_NAMED => {
                let key = ArrayKey::Str(name.clone());
                match array.get_mut(&key) {
                    Some(cur) => {
                        if let Value::Array(a) = cur {
                            let _ = Rc::make_mut(a).append(val);
                        } else {
                            let old = std::mem::take(cur);
                            let mut a = PhpArray::new();
                            let _ = a.append(old);
                            let _ = a.append(val);
                            *cur = Value::array(a);
                        }
                    }
                    None => {
                        array.insert(key, val);
                    }
                }
            }
            FETCH_NUM => {
                let _ = array.append(val);
            }
            FETCH_CLASS | FETCH_OBJ | FETCH_INTO => {
                let obj = object.clone().expect("object fetch modes create an object");
                props::update_property(vm, scope.as_ref(), &obj, &name, val)?;
            }
            FETCH_FUNC => func_params.push(val),
            _ => {}
        }
        column_index += 1;
    }

    if how == FETCH_CLASS && flags & (FETCH_PROPS_LATE | FETCH_SERIALIZE) == 0 {
        if let Some(obj) = &object {
            if obj.class().magic.constructor.is_some() {
                call_constructor(vm, obj, ctor_args.as_ref())?;
            }
        }
    } else if how == FETCH_FUNC {
        let f = func.expect("checked above");
        let r = vm.call(&f, func_params)?;
        return Ok(Some(r.into_unref()));
    }

    Ok(Some(match object {
        Some(o) => Value::Object(o),
        None => Value::array(array),
    }))
}

/// `pdo_verify_fetch_mode`.
fn verify_fetch_mode(
    vm: &mut Vm,
    default_mode: i64,
    mode_and_flags: i64,
    arg_num: usize,
    fetch_all: bool,
) -> PResult<()> {
    if !(0..FIRST_INVALID_FLAG).contains(&mode_and_flags) {
        return Err(vm.argument_value_error(arg_num, "must be a bitmask of PDO::FETCH_* constants"));
    }
    let mut flags = mode_and_flags & FETCH_FLAGS;
    let mut mode = mode_and_flags & !FETCH_FLAGS;
    if mode == FETCH_USE_DEFAULT {
        flags = default_mode & FETCH_FLAGS;
        mode = default_mode & !FETCH_FLAGS;
    }
    if flags != 0 {
        let class_flags = flags & (FETCH_CLASSTYPE | FETCH_PROPS_LATE | FETCH_SERIALIZE) != 0;
        if class_flags && mode != FETCH_CLASS {
            return Err(vm.argument_value_error(
                arg_num,
                "cannot use PDO::FETCH_CLASSTYPE, PDO::FETCH_PROPS_LATE, or PDO::FETCH_SERIALIZE fetch flags with a fetch mode other than PDO::FETCH_CLASS",
            ));
        }
        if flags & FETCH_SERIALIZE != 0 {
            vm.deprecated("The PDO::FETCH_SERIALIZE mode is deprecated")?;
        }
    }
    match mode {
        FETCH_FUNC => {
            if !fetch_all {
                return Err(vm.argument_value_error(
                    arg_num,
                    "PDO::FETCH_FUNC can only be used with PDOStatement::fetchAll()",
                ));
            }
            Ok(())
        }
        FETCH_LAZY => {
            if fetch_all {
                return Err(vm.argument_value_error(
                    arg_num,
                    "PDO::FETCH_LAZY cannot be used with PDOStatement::fetchAll()",
                ));
            }
            Ok(())
        }
        FETCH_INTO => {
            if fetch_all {
                return Err(vm.argument_value_error(
                    arg_num,
                    "PDO::FETCH_INTO cannot be used with PDOStatement::fetchAll()",
                ));
            }
            Ok(())
        }
        FETCH_ASSOC | FETCH_NUM | FETCH_BOTH | FETCH_OBJ | FETCH_BOUND | FETCH_COLUMN
        | FETCH_CLASS | FETCH_NAMED | FETCH_KEY_PAIR => Ok(()),
        _ => Err(vm.argument_value_error(arg_num, "must be a bitmask of PDO::FETCH_* constants")),
    }
}

fn count_error(vm: &mut Vm, word: &str, n: usize, given: usize) -> Flow {
    let func = vm.current_function_display_name();
    vm.argument_count_error(format!(
        "{func}() expects {word} {n} arguments for the fetch mode provided, {given} given"
    ))
}

/// `pdo_stmt_setup_fetch_mode`: validates and stores a default fetch mode
/// with its arguments (`setFetchMode()`, `PDO::query()`).
pub fn setup_fetch_mode(
    vm: &mut Vm,
    stmt: &Stmt,
    dbh: &Dbh,
    mode: i64,
    mode_arg_num: usize,
    args: &[Value],
) -> PResult<()> {
    let arg1_num = mode_arg_num + 1;
    let ctor_arg_num = mode_arg_num + 2;
    let nargs = args.len();
    let total = mode_arg_num + nargs;
    let fetch_type = mode & !FETCH_FLAGS;
    let flags = mode & FETCH_FLAGS;

    verify_fetch_mode(
        vm,
        dbh.default_fetch_type.get() as i64,
        mode,
        mode_arg_num,
        false,
    )?;

    let mut new = FetchState::default();
    match fetch_type {
        FETCH_USE_DEFAULT | FETCH_LAZY | FETCH_ASSOC | FETCH_NUM | FETCH_BOTH | FETCH_OBJ
        | FETCH_BOUND | FETCH_NAMED | FETCH_KEY_PAIR => {
            if nargs != 0 {
                return Err(count_error(vm, "exactly", mode_arg_num, total));
            }
        }
        FETCH_COLUMN => {
            if nargs != 1 {
                return Err(count_error(vm, "exactly", arg1_num, total));
            }
            let Value::Int(col) = args[0].unref() else {
                let msg = format!(
                    "must be of type int, {} given",
                    args[0].unref().value_name()
                );
                return Err(vm.argument_type_error(arg1_num, &msg));
            };
            if col < 0 {
                return Err(vm.argument_value_error(arg1_num, "must be greater than or equal to 0"));
            }
            new.column = col;
        }
        FETCH_CLASS => {
            if flags & FETCH_CLASSTYPE == FETCH_CLASSTYPE {
                if nargs != 0 {
                    return Err(count_error(vm, "exactly", mode_arg_num, total));
                }
            } else {
                if nargs == 0 {
                    return Err(count_error(vm, "at least", arg1_num, total));
                }
                if nargs > 2 {
                    return Err(count_error(vm, "at most", ctor_arg_num, total));
                }
                let Value::String(name) = args[0].unref() else {
                    let msg = format!(
                        "must be of type string, {} given",
                        args[0].unref().value_name()
                    );
                    return Err(vm.argument_type_error(arg1_num, &msg));
                };
                let Some(cep) = vm.lookup_class(&name)? else {
                    return Err(vm.argument_type_error(arg1_num, "must be a valid class"));
                };
                if nargs == 2 {
                    match args[1].unref() {
                        Value::Null => {}
                        Value::Array(a) => {
                            if !a.is_empty() {
                                if cep.magic.constructor.is_none() {
                                    return Err(vm.argument_value_error(
                                        3,
                                        "must be empty when class provided in argument #2 ($class) does not have a constructor",
                                    ));
                                }
                                new.cls_ctor_args = Some(a.clone());
                            }
                        }
                        other => {
                            let msg =
                                format!("must be of type ?array, {} given", other.value_name());
                            return Err(vm.argument_type_error(ctor_arg_num, &msg));
                        }
                    }
                }
                new.cls_ce = Some(cep);
            }
        }
        FETCH_INTO => {
            if total != arg1_num {
                return Err(count_error(vm, "exactly", arg1_num, total));
            }
            let Value::Object(o) = args[0].unref() else {
                let msg = format!(
                    "must be of type object, {} given",
                    args[0].unref().value_name()
                );
                return Err(vm.argument_type_error(arg1_num, &msg));
            };
            new.into = Some(o);
        }
        _ => {
            return Err(
                vm.argument_value_error(mode_arg_num, "must be one of the PDO::FETCH_* constants")
            );
        }
    }
    *stmt.fetch.borrow_mut() = new;
    if fetch_type == FETCH_USE_DEFAULT {
        stmt.default_fetch_type
            .set(dbh.default_fetch_type.get() as i64);
    } else {
        stmt.default_fetch_type.set(mode);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Statement construction (shared with PDO::prepare()/query())
// ---------------------------------------------------------------------------

/// `pdo_stmt_instantiate`.
pub fn instantiate(vm: &mut Vm, ce: &ClassRef, ctor_args: Option<&PhpArrayRef>) -> PResult<ObjRef> {
    if ctor_args.is_some() && ce.magic.constructor.is_none() {
        return Err(vm.throw_error(
            "Error",
            "User-supplied statement does not accept constructor arguments",
        ));
    }
    let obj = object_init_ex(vm, ce)?;
    if stmt_of(&obj).is_none() {
        obj.set_native(StmtHandle(Rc::new(Stmt::new())));
    }
    Ok(obj)
}

/// `pdo_stmt_construct`: sets `queryString` and runs the constructor.
pub fn construct(
    vm: &mut Vm,
    obj: &ObjRef,
    stmt: &Stmt,
    ctor_args: Option<&PhpArrayRef>,
) -> PResult<()> {
    obj.set_prop(b"queryString", Value::String(stmt.query_string()));
    if obj.class().magic.constructor.is_some() {
        call_constructor(vm, obj, ctor_args)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

macro_rules! method {
    ($name:ident, $imp:ident) => {
        fn $name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            let dbh = args.this().and_then(stmt_of).and_then(|s| s.dbh());
            let r = $imp(vm, args);
            finish(vm, dbh.as_ref(), r)
        }
    };
}

fn execute_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array_opt(0).cloned();
    let (_, stmt, dbh) = get_obj(vm, args)?;
    disallow_reentrant_param_event(vm, &stmt)?;
    if stmt.executed.get() && stmt.driver_supports(|d| d.supports_cursor_closer()) {
        stmt.with_driver(vm, |d, vm| d.close_cursor(vm, &stmt))?;
    }
    stmt.execute_incomplete.set(true);
    stmt.clear_err();
    if let Some(input) = input {
        *stmt.bound_params.borrow_mut() = None;
        for (k, v) in input.iter() {
            let mut param = BoundParam::new(v.unref(), PARAM_STR as i32);
            match k {
                ArrayKey::Str(s) => {
                    param.name = Some(s.clone());
                    param.paramno = -1;
                }
                ArrayKey::Int(i) => param.paramno = i,
            }
            if !register_bound_param(vm, &stmt, &dbh, param, true)? {
                return Ok(Value::Bool(false));
            }
        }
    }
    let mut ret = true;
    if stmt.supports_placeholders.get() == PLACEHOLDER_NONE {
        *stmt.active_query_string.borrow_mut() = ActiveQuery::None;
        let query = stmt.query_string();
        match parser::parse_params(vm, &stmt, &dbh, &query)? {
            parser::Parsed::Unchanged => {
                *stmt.active_query_string.borrow_mut() = ActiveQuery::SameAsQuery;
            }
            parser::Parsed::Rewritten(q) => {
                *stmt.active_query_string.borrow_mut() = ActiveQuery::Rewritten(q);
            }
            parser::Parsed::Error => return Ok(Value::Bool(false)),
        }
    } else if !dispatch_param_event(vm, &stmt, &dbh, ParamEvent::ExecPre)? {
        handle_stmt_err(vm, &dbh, &stmt)?;
        return Ok(Value::Bool(false));
    }
    if stmt.with_driver(vm, |d, vm| d.execute(vm, &stmt))? {
        stmt.execute_incomplete.set(false);
        if !stmt.executed.get() {
            if dbh.alloc_own_columns.get() && stmt.columns.borrow().is_none() {
                ret = describe_columns(vm, &stmt, &dbh)?;
            }
            stmt.executed.set(true);
        }
        if ret && !dispatch_param_event(vm, &stmt, &dbh, ParamEvent::ExecPost)? {
            handle_stmt_err(vm, &dbh, &stmt)?;
            return Ok(Value::Bool(false));
        }
        return Ok(Value::Bool(ret));
    }
    handle_stmt_err(vm, &dbh, &stmt)?;
    Ok(Value::Bool(false))
}
method!(execute, execute_impl);

fn fetch_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (how, ori, off) = (args.int(0), args.int(1), args.int(2));
    let (this, stmt, dbh) = get_obj(vm, args)?;
    stmt.clear_err();
    verify_fetch_mode(vm, stmt.default_fetch_type.get(), how, 1, false)?;
    if how & !FETCH_FLAGS == FETCH_KEY_PAIR {
        let mut arr = PhpArray::new();
        if !key_pair_fetch(vm, &stmt, &dbh, ori, off, &mut arr)? {
            handle_stmt_err(vm, &dbh, &stmt)?;
            return Ok(Value::Bool(false));
        }
        return Ok(Value::array(arr));
    }
    match do_fetch(vm, &this, &stmt, &dbh, how, ori, off, None)? {
        Some(v) => Ok(v),
        None => {
            handle_stmt_err(vm, &dbh, &stmt)?;
            Ok(Value::Bool(false))
        }
    }
}
method!(fetch, fetch_impl);

/// `Z_PARAM_CLASS_OR_NULL`.
fn class_param(vm: &mut Vm, args: &Args, i: usize) -> PResult<Option<ClassRef>> {
    match args.str_opt(i) {
        None => Ok(None),
        Some(name) => {
            let name = name.clone();
            match vm.lookup_class(&name)? {
                Some(ce) => Ok(Some(ce)),
                None => Err(vm.argument_type_error(
                    i + 1,
                    &format!("must be a valid class name, {name} given"),
                )),
            }
        }
    }
}

fn fetch_object_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ce = class_param(vm, args, 0)?;
    let ctor_args = args.array(1).clone();
    let (this, stmt, dbh) = get_obj(vm, args)?;
    stmt.clear_err();
    let ce = match ce {
        Some(ce) => ce,
        None => super::find_class(vm, "stdClass"),
    };
    if !ctor_args.is_empty() && ce.magic.constructor.is_none() {
        return Err(vm.argument_value_error(
            2,
            "must be empty when class provided in argument #1 ($class) does not have a constructor",
        ));
    }
    let old = stmt.fetch.borrow().clone();
    {
        let mut f = stmt.fetch.borrow_mut();
        f.cls_ce = Some(ce);
        f.cls_ctor_args = Some(ctor_args);
    }
    let r = do_fetch(vm, &this, &stmt, &dbh, FETCH_CLASS, FETCH_ORI_NEXT, 0, None);
    {
        let mut f = stmt.fetch.borrow_mut();
        f.cls_ce = old.cls_ce;
        f.cls_ctor_args = old.cls_ctor_args;
    }
    match r? {
        Some(v) => Ok(v),
        None => {
            handle_stmt_err(vm, &dbh, &stmt)?;
            Ok(Value::Bool(false))
        }
    }
}
method!(fetch_object, fetch_object_impl);

fn fetch_column_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let col = args.int(0);
    let (_, stmt, dbh) = get_obj(vm, args)?;
    stmt.clear_err();
    if !do_fetch_common(vm, &stmt, &dbh, FETCH_ORI_NEXT, 0)? {
        handle_stmt_err(vm, &dbh, &stmt)?;
        return Ok(Value::Bool(false));
    }
    fetch_value(vm, &stmt, &dbh, col, None)
}
method!(fetch_column, fetch_column_impl);

fn fetch_all_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let num_args = args.len();
    if num_args > 3 {
        return Err(vm.argument_count_error(format!(
            "PDOStatement::fetchAll() expects at most 3 arguments, {num_args} given"
        )));
    }
    let mut how = args.int(0);
    let rest = args.rest(1).to_vec();
    let arg2 = rest.first().map(Value::unref).filter(|v| !v.is_null());
    let ctor_args = match rest.get(1).map(Value::unref) {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a),
        Some(other) => {
            let msg = format!("must be of type ?array, {} given", other.value_name());
            return Err(vm.argument_type_error(3, &msg));
        }
    };
    let (this, stmt, dbh) = get_obj(vm, args)?;
    verify_fetch_mode(vm, stmt.default_fetch_type.get(), how, 1, true)?;
    let mut fetch_mode = how & !FETCH_FLAGS;
    let mut flags = how & FETCH_FLAGS;
    let old = stmt.fetch.borrow().clone();

    match fetch_mode {
        FETCH_CLASS => {
            let fetch_class = match &arg2 {
                Some(Value::String(name)) => match vm.lookup_class(name)? {
                    Some(ce) => ce,
                    None => return Err(vm.argument_type_error(2, "must be a valid class")),
                },
                Some(other) => {
                    let msg = format!("must be of type string, {} given", other.value_name());
                    return Err(vm.argument_type_error(2, &msg));
                }
                None => super::find_class(vm, "stdClass"),
            };
            if let Some(a) = ctor_args.as_ref().filter(|a| !a.is_empty()) {
                if fetch_class.magic.constructor.is_none() {
                    return Err(vm.argument_value_error(
                        3,
                        "must be empty when class provided in argument #2 ($class) does not have a constructor",
                    ));
                }
                stmt.fetch.borrow_mut().cls_ctor_args = Some(a.clone());
            }
            stmt.fetch.borrow_mut().cls_ce = Some(fetch_class);
        }
        FETCH_FUNC => {
            if num_args != 2 {
                return Err(vm.argument_count_error(format!(
                    "PDOStatement::fetchAll() expects exactly 2 argument for PDO::FETCH_FUNC, {num_args} given"
                )));
            }
            let Some(callable) = arg2.clone() else {
                return Err(vm.argument_type_error(2, "must be a callable, null given"));
            };
            let host = vm.host.clone();
            if let Err(e) = host.check_callable(vm, &callable) {
                return Err(vm.throw_type_error(e));
            }
            stmt.fetch.borrow_mut().func = Some(callable);
        }
        FETCH_COLUMN => {
            if num_args > 2 {
                return Err(vm.argument_count_error(format!(
                    "PDOStatement::fetchAll() expects at most 2 argument for the fetch mode provided, {num_args} given"
                )));
            }
            match &arg2 {
                Some(Value::Int(c)) => {
                    if *c < 0 {
                        return Err(
                            vm.argument_value_error(2, "must be greater than or equal to 0")
                        );
                    }
                    stmt.fetch.borrow_mut().column = *c;
                }
                Some(other) => {
                    let msg = format!("must be of type int, {} given", other.value_name());
                    return Err(vm.argument_type_error(2, &msg));
                }
                None => {
                    stmt.fetch.borrow_mut().column = if flags & (FETCH_GROUP | FETCH_UNIQUE) != 0 {
                        -1
                    } else {
                        0
                    };
                }
            }
        }
        _ => {
            if num_args > 1 {
                return Err(vm.argument_count_error(format!(
                    "PDOStatement::fetchAll() expects exactly 1 argument for the fetch mode provided, {num_args} given"
                )));
            }
        }
    }

    if fetch_mode == FETCH_USE_DEFAULT {
        let def = stmt.default_fetch_type.get();
        flags |= def & FETCH_FLAGS;
        fetch_mode = def & !FETCH_FLAGS;
        how = fetch_mode | flags;
    }

    stmt.clear_err();
    let mut result = PhpArray::new();
    let restore = |stmt: &Stmt| {
        let mut f = stmt.fetch.borrow_mut();
        f.cls_ce = old.cls_ce.clone();
        f.cls_ctor_args = old.cls_ctor_args.clone();
    };
    if fetch_mode == FETCH_KEY_PAIR {
        loop {
            match key_pair_fetch(vm, &stmt, &dbh, FETCH_ORI_NEXT, 0, &mut result) {
                Ok(true) => continue,
                Ok(false) => break,
                Err(e) => {
                    restore(&stmt);
                    return Err(e);
                }
            }
        }
        handle_stmt_err(vm, &dbh, &stmt)?;
        return Ok(Value::array(result));
    }
    let r = (|| -> PResult<()> {
        if flags & (FETCH_GROUP | FETCH_UNIQUE) != 0 {
            loop {
                let mut group_key = Value::Undef;
                let Some(data) = do_fetch(
                    vm,
                    &this,
                    &stmt,
                    &dbh,
                    how | flags,
                    FETCH_ORI_NEXT,
                    0,
                    Some(&mut group_key),
                )?
                else {
                    break;
                };
                let key_bytes = match &group_key {
                    Value::String(s) => s.clone(),
                    _ => PhpStr::new(),
                };
                let key = ArrayKey::from_bytes(&key_bytes);
                if flags & FETCH_UNIQUE == FETCH_UNIQUE {
                    result.insert(key, data);
                } else {
                    let slot = result.entry_mut(key);
                    if !matches!(slot, Value::Array(_)) {
                        *slot = Value::empty_array();
                    }
                    if let Value::Array(a) = slot {
                        let _ = Rc::make_mut(a).append(data);
                    }
                }
            }
        } else {
            while let Some(data) = do_fetch(vm, &this, &stmt, &dbh, how, FETCH_ORI_NEXT, 0, None)? {
                let _ = result.append(data);
            }
        }
        Ok(())
    })();
    restore(&stmt);
    r?;
    if stmt.error_code.get() != ERR_NONE {
        error::handle_error(vm, &dbh, Some(&stmt))?;
        return Ok(Value::empty_array());
    }
    Ok(Value::array(result))
}
method!(fetch_all, fetch_all_impl);

/// `register_bound_param` for `bindParam()`/`bindColumn()`.
fn bind_common(vm: &mut Vm, args: &mut Args, is_param: bool) -> PResult<Value> {
    let key = args.get(0).clone();
    let var = args.get(1).clone();
    let param_type = args.int(2);
    let max_len = args.int(3);
    let driver_params = args.opt(4).cloned();
    let (_, stmt, dbh) = get_obj(vm, args)?;
    disallow_reentrant_param_event(vm, &stmt)?;
    let mut param = BoundParam::new(var, param_type as i32);
    param.max_value_len = max_len;
    match key {
        Value::String(name) => {
            if name.is_empty() {
                return Err(vm.argument_value_error(1, "must not be empty"));
            }
            param.name = Some(name);
            param.paramno = -1;
        }
        other => {
            let n = to_int_silent(&other);
            if n > 0 {
                param.paramno = n - 1;
            } else {
                return Err(vm.argument_value_error(1, "must be greater than or equal to 1"));
            }
        }
    }
    if let Some(dp) = driver_params {
        param.driver_params = dp;
    }
    Ok(Value::Bool(register_bound_param(
        vm, &stmt, &dbh, param, is_param,
    )?))
}

fn bind_param_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bind_common(vm, args, true)
}
method!(bind_param, bind_param_impl);

fn bind_column_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bind_common(vm, args, false)
}
method!(bind_column, bind_column_impl);

fn bind_value_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let key = args.get(0).clone();
    let value = args.get(1).unref();
    let param_type = args.int(2);
    let (_, stmt, dbh) = get_obj(vm, args)?;
    disallow_reentrant_param_event(vm, &stmt)?;
    let mut param = BoundParam::new(value, param_type as i32);
    match key {
        Value::String(name) => {
            if name.is_empty() {
                return Err(vm.argument_value_error(1, "must not be empty"));
            }
            param.name = Some(name);
            param.paramno = -1;
        }
        other => {
            let n = to_int_silent(&other);
            if n > 0 {
                param.paramno = n - 1;
            } else {
                return Err(vm.argument_value_error(1, "must be greater than or equal to 1"));
            }
        }
    }
    Ok(Value::Bool(register_bound_param(
        vm, &stmt, &dbh, param, true,
    )?))
}
method!(bind_value, bind_value_impl);

fn row_count_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, stmt, _) = get_obj(vm, args)?;
    Ok(Value::Int(stmt.row_count.get()))
}
method!(row_count, row_count_impl);

fn error_code_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, stmt, _) = get_obj(vm, args)?;
    let code = stmt.error_code.get();
    if code[0] == 0 {
        return Ok(Value::Null);
    }
    Ok(Value::String(err_str(&code)))
}
method!(error_code, error_code_impl);

fn error_info_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, stmt, dbh) = get_obj(vm, args)?;
    let code = stmt.error_code.get();
    let mut arr = PhpArray::new();
    let _ = arr.append(Value::String(err_str(&code)));
    if err_str(&code).as_bytes() != b"00000" {
        if let Some(m) = dbh.methods().filter(|m| m.supports_fetch_err()) {
            m.fetch_err(&dbh, Some(&stmt), &mut arr);
        }
    }
    while arr.len() < 3 {
        let _ = arr.append(Value::Null);
    }
    Ok(Value::array(arr))
}
method!(error_info, error_info_impl);

fn set_attribute_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let attr = args.int(0);
    let value = args.get(1).clone();
    let (_, stmt, dbh) = get_obj(vm, args)?;
    if !stmt.driver_supports(|d| d.supports_set_attribute()) {
        raise_impl_error(
            vm,
            &dbh,
            Some(&stmt),
            "IM001",
            Some("This driver doesn't support setting attributes"),
        )?;
        return Ok(Value::Bool(false));
    }
    stmt.clear_err();
    if stmt.with_driver(vm, |d, vm| d.set_attribute(vm, &stmt, attr, &value))? {
        return Ok(Value::Bool(true));
    }
    handle_stmt_err(vm, &dbh, &stmt)?;
    Ok(Value::Bool(false))
}
method!(set_attribute, set_attribute_impl);

/// `generic_stmt_attr_get`.
fn generic_attr(stmt: &Stmt, attr: i64) -> Option<Value> {
    (attr == ATTR_EMULATE_PREPARES)
        .then(|| Value::Bool(stmt.supports_placeholders.get() == PLACEHOLDER_NONE))
}

fn get_attribute_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let attr = args.int(0);
    let (_, stmt, dbh) = get_obj(vm, args)?;
    if !stmt.driver_supports(|d| d.supports_get_attribute()) {
        if let Some(v) = generic_attr(&stmt, attr) {
            return Ok(v);
        }
        raise_impl_error(
            vm,
            &dbh,
            Some(&stmt),
            "IM001",
            Some("This driver doesn't support getting attributes"),
        )?;
        return Ok(Value::Bool(false));
    }
    stmt.clear_err();
    match stmt.with_driver(vm, |d, vm| d.get_attribute(vm, &stmt, attr))? {
        AttrValue::Error => {
            handle_stmt_err(vm, &dbh, &stmt)?;
            Ok(Value::Bool(false))
        }
        AttrValue::Unsupported => {
            if let Some(v) = generic_attr(&stmt, attr) {
                return Ok(v);
            }
            raise_impl_error(
                vm,
                &dbh,
                Some(&stmt),
                "IM001",
                Some("driver doesn't support getting that attribute"),
            )?;
            Ok(Value::Bool(false))
        }
        AttrValue::Value(v) => Ok(v),
    }
}
method!(get_attribute, get_attribute_impl);

fn column_count_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, stmt, _) = get_obj(vm, args)?;
    Ok(Value::Int(stmt.column_count.get() as i64))
}
method!(column_count, column_count_impl);

fn get_column_meta_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let colno = args.int(0);
    let (_, stmt, dbh) = get_obj(vm, args)?;
    if colno < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    if !stmt.driver_supports(|d| d.supports_column_meta()) {
        raise_impl_error(
            vm,
            &dbh,
            Some(&stmt),
            "IM001",
            Some("driver doesn't support meta data"),
        )?;
        return Ok(Value::Bool(false));
    }
    stmt.clear_err();
    let Some(mut meta) = stmt.with_driver(vm, |d, vm| d.column_meta(vm, &stmt, colno))? else {
        handle_stmt_err(vm, &dbh, &stmt)?;
        return Ok(Value::Bool(false));
    };
    let col = stmt
        .columns
        .borrow()
        .as_ref()
        .filter(|_| colno < stmt.column_count.get() as i64)
        .and_then(|c| c.get(colno as usize).cloned());
    let Some(col) = col else {
        raise_impl_error(vm, &dbh, Some(&stmt), "07009", Some("invalid column index"))?;
        return Ok(Value::Bool(false));
    };
    meta.insert_str("name", Value::String(col.name));
    meta.insert_str("len", Value::Int(col.maxlen));
    meta.insert_str("precision", Value::Int(col.precision as i64));
    Ok(Value::array(meta))
}
method!(get_column_meta, get_column_meta_impl);

fn set_fetch_mode_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(0);
    let rest: Vec<Value> = args.rest(1).to_vec();
    let (_, stmt, dbh) = get_obj(vm, args)?;
    if stmt.in_fetch.get() {
        return Err(vm.throw_error("Error", "Cannot change default fetch mode while fetching"));
    }
    setup_fetch_mode(vm, &stmt, &dbh, mode, 1, &rest)?;
    Ok(Value::Bool(true))
}
method!(set_fetch_mode, set_fetch_mode_impl);

/// `pdo_stmt_do_next_rowset`.
fn do_next_rowset(vm: &mut Vm, stmt: &Stmt, dbh: &Dbh) -> PResult<bool> {
    stmt.reset_columns();
    if !stmt.with_driver(vm, |d, vm| d.next_rowset(vm, stmt))? {
        stmt.executed.set(false);
        return Ok(false);
    }
    describe_columns(vm, stmt, dbh)?;
    Ok(true)
}

fn next_rowset_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, stmt, dbh) = get_obj(vm, args)?;
    if !stmt.driver_supports(|d| d.supports_next_rowset()) {
        raise_impl_error(
            vm,
            &dbh,
            Some(&stmt),
            "IM001",
            Some("driver does not support multiple rowsets"),
        )?;
        return Ok(Value::Bool(false));
    }
    stmt.clear_err();
    if !do_next_rowset(vm, &stmt, &dbh)? {
        handle_stmt_err(vm, &dbh, &stmt)?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}
method!(next_rowset, next_rowset_impl);

fn close_cursor_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, stmt, dbh) = get_obj(vm, args)?;
    disallow_reentrant_param_event(vm, &stmt)?;
    if !stmt.driver_supports(|d| d.supports_cursor_closer()) {
        // Emulate it by fetching and discarding rows.
        loop {
            while stmt.with_driver(vm, |d, vm| d.fetch(vm, &stmt, FETCH_ORI_NEXT, 0))? {}
            if !stmt.driver_supports(|d| d.supports_next_rowset()) {
                break;
            }
            if !do_next_rowset(vm, &stmt, &dbh)? {
                break;
            }
        }
        stmt.executed.set(false);
        return Ok(Value::Bool(true));
    }
    stmt.clear_err();
    if !stmt.with_driver(vm, |d, vm| d.close_cursor(vm, &stmt))? {
        handle_stmt_err(vm, &dbh, &stmt)?;
        return Ok(Value::Bool(false));
    }
    stmt.executed.set(false);
    Ok(Value::Bool(true))
}
method!(close_cursor, close_cursor_impl);

fn debug_dump_params_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (_, stmt, _) = get_obj(vm, args)?;
    let mut out: Vec<u8> = Vec::new();
    let query = stmt.query_string();
    out.extend_from_slice(format!("SQL: [{}] ", query.len()).as_bytes());
    out.extend_from_slice(&query);
    out.push(b'\n');
    if let ActiveQuery::Rewritten(q) = &*stmt.active_query_string.borrow() {
        out.extend_from_slice(format!("Sent SQL: [{}] ", q.len()).as_bytes());
        out.extend_from_slice(q);
        out.push(b'\n');
    }
    let params = stmt.bound_params.borrow().clone();
    out.extend_from_slice(
        format!("Params:  {}\n", params.as_ref().map_or(0, ParamTable::len)).as_bytes(),
    );
    if let Some(params) = params {
        for (key, p) in params.iter() {
            match key {
                ParamKey::Str(k) => {
                    out.extend_from_slice(format!("Key: Name: [{}] ", k.len()).as_bytes());
                    out.extend_from_slice(k);
                    out.push(b'\n');
                }
                ParamKey::Int(n) => {
                    out.extend_from_slice(format!("Key: Position #{}:\n", *n as u64).as_bytes());
                }
            }
            let name = p.name.clone().unwrap_or_default();
            out.extend_from_slice(
                format!("paramno={}\nname=[{}] \"", p.paramno, name.len()).as_bytes(),
            );
            out.extend_from_slice(&name);
            out.extend_from_slice(
                format!(
                    "\"\nis_param={}\nparam_type={}\n",
                    p.is_param as i32, p.param_type
                )
                .as_bytes(),
            );
        }
    }
    vm.echo(&out)?;
    Ok(Value::Null)
}
method!(debug_dump_params, debug_dump_params_impl);

fn get_iterator_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = args
        .this()
        .cloned()
        .expect("PDOStatement method without $this");
    iterator::create(vm, &this).map(Value::Object)
}
method!(get_iterator, get_iterator_impl);

/// `dbstmt_prop_write`: `queryString` is read-only once set.
fn prop_write(vm: &mut Vm, obj: &ObjRef, name: &PhpStr, _value: Value) -> PResult<bool> {
    if name.as_bytes() == b"queryString" && obj.get_prop(b"queryString").is_some() {
        return Err(vm.throw_error("Error", "Property queryString is read only"));
    }
    Ok(false)
}

fn create_stmt(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(StmtHandle(Rc::new(Stmt::new())));
    Ok(obj)
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

macro_rules! natives {
    ($($id:ident: $sig:literal => $h:ident;)*) => {
        $(static $id: NativeFunction = NativeFunction { sig: $sig, handler: $h };)*
    };
}

natives! {
    BIND_COLUMN: "public function bindColumn(string|int $column, mixed &$var, int $type = PDO::PARAM_STR, int $maxLength = 0, mixed $driverOptions = null): bool" => bind_column;
    BIND_PARAM: "public function bindParam(string|int $param, mixed &$var, int $type = PDO::PARAM_STR, int $maxLength = 0, mixed $driverOptions = null): bool" => bind_param;
    BIND_VALUE: "public function bindValue(string|int $param, mixed $value, int $type = PDO::PARAM_STR): bool" => bind_value;
    CLOSE_CURSOR: "public function closeCursor(): bool" => close_cursor;
    COLUMN_COUNT: "public function columnCount(): int" => column_count;
    DEBUG_DUMP_PARAMS: "public function debugDumpParams(): ?bool" => debug_dump_params;
    ERROR_CODE: "public function errorCode(): ?string" => error_code;
    ERROR_INFO: "public function errorInfo(): array" => error_info;
    EXECUTE: "public function execute(?array $params = null): bool" => execute;
    FETCH: "public function fetch(int $mode = PDO::FETCH_DEFAULT, int $cursorOrientation = PDO::FETCH_ORI_NEXT, int $cursorOffset = 0): mixed" => fetch;
    FETCH_ALL: "public function fetchAll(int $mode = PDO::FETCH_DEFAULT, mixed ...$args): array" => fetch_all;
    FETCH_COLUMN_FN: "public function fetchColumn(int $column = 0): mixed" => fetch_column;
    FETCH_OBJECT: "public function fetchObject(?string $class = \"stdClass\", array $constructorArgs = []): object|false" => fetch_object;
    GET_ATTRIBUTE: "public function getAttribute(int $name): mixed" => get_attribute;
    GET_COLUMN_META: "public function getColumnMeta(int $column): array|false" => get_column_meta;
    NEXT_ROWSET: "public function nextRowset(): bool" => next_rowset;
    ROW_COUNT: "public function rowCount(): int" => row_count;
    SET_ATTRIBUTE: "public function setAttribute(int $attribute, mixed $value): bool" => set_attribute;
    SET_FETCH_MODE: "public function setFetchMode(int $mode, mixed ...$args): true" => set_fetch_mode;
    GET_ITERATOR: "public function getIterator(): Iterator" => get_iterator;
}

/// Declares `public string $queryString;` (uninitialised).
pub fn declare_query_string(ce: &mut ClassEntry, class: &str) {
    let mut p = PropertyInfo::new("queryString", class, member_flags::PUBLIC);
    p.ty = Some(TypeDecl::parse("string").expect("valid type"));
    p.default = Value::Undef;
    ce.declare_property(p);
}

/// Registers `PDOStatement`.
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(STMT_CLASS, class_flags::NOT_SERIALIZABLE);
    if let Some(agg) = r.find_class("IteratorAggregate") {
        ce.interfaces.extend(agg.interfaces.iter().cloned());
        ce.interfaces.push(agg);
    }
    declare_query_string(&mut ce, STMT_CLASS);
    for nf in [
        &BIND_COLUMN,
        &BIND_PARAM,
        &BIND_VALUE,
        &CLOSE_CURSOR,
        &COLUMN_COUNT,
        &DEBUG_DUMP_PARAMS,
        &ERROR_CODE,
        &ERROR_INFO,
        &EXECUTE,
        &FETCH,
        &FETCH_ALL,
        &FETCH_COLUMN_FN,
        &FETCH_OBJECT,
        &GET_ATTRIBUTE,
        &GET_COLUMN_META,
        &NEXT_ROWSET,
        &ROW_COUNT,
        &SET_ATTRIBUTE,
        &SET_FETCH_MODE,
        &GET_ITERATOR,
    ] {
        ce.add_method(Function::native_method(
            nf,
            STMT_CLASS,
            member_flags::PUBLIC,
        ));
    }
    ce.handlers.create_object = Some(create_stmt);
    ce.handlers.clone_obj = Some(clone_forbidden);
    ce.handlers.write_property = Some(prop_write);
    r.class(ce);
}
