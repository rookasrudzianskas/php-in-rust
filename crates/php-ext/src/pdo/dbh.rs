//! The `PDO` class (`pdo_dbh.c`): connection handles, attributes,
//! transactions and statement creation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use php_runtime::class::{ClassConstant, ClassEntry, Function, class_flags, member_flags};
use php_runtime::value::convert::to_int_silent;
use php_runtime::value::numeric::{Numeric, is_numeric_str};
use php_runtime::{
    Args, ArrayKey, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpArrayRef, PhpStr,
    Registry, Value, Vm,
};

use super::driver::{AttrValue, DbhMethods, Driver};
use super::error::{
    self, ERR_NONE, ErrCode, err_str, handle_dbh_err, handle_stmt_err, raise_impl_error,
};
use super::stmt::{self, ActiveQuery, STMT_CLASS, Stmt};
use super::{DRIVERS, PdoGlobals, consts::*, finish};

pub const PDO_CLASS: &str = "PDO";

/// `enum pdo_error_mode`.
pub const ERRMODE_SILENT: u8 = 0;
pub const ERRMODE_WARNING: u8 = 1;
pub const ERRMODE_EXCEPTION: u8 = 2;

/// `enum pdo_case_conversion`.
pub const CASE_NATURAL: u8 = 0;
pub const CASE_UPPER: u8 = 1;
pub const CASE_LOWER: u8 = 2;

/// `enum pdo_null_handling`.
pub const NULL_NATURAL: u8 = 0;
pub const NULL_EMPTY_STRING: u8 = 1;
pub const NULL_TO_STRING: u8 = 2;

/// `pdo_dbh_t`.
pub struct Dbh {
    /// Driver operations and driver data (`methods` + `driver_data`).
    pub methods: RefCell<Option<Rc<dyn DbhMethods>>>,
    /// Set once the factory succeeded (`PDO_CONSTRUCT_CHECK`).
    pub driver: Cell<Option<&'static dyn Driver>>,
    /// The DSN after `driver:`.
    pub data_source: RefCell<PhpStr>,
    pub username: RefCell<Option<PhpStr>>,
    pub password: RefCell<Option<PhpStr>>,
    pub is_persistent: Cell<bool>,
    pub auto_commit: Cell<bool>,
    pub is_closed: Cell<bool>,
    pub alloc_own_columns: Cell<bool>,
    pub in_txn: Cell<bool>,
    pub stringify: Cell<bool>,
    /// Bitmap of parameter events the driver does not need.
    pub skip_param_evt: Cell<u8>,
    pub error_mode: Cell<u8>,
    pub oracle_nulls: Cell<u8>,
    pub native_case: Cell<u8>,
    pub desired_case: Cell<u8>,
    pub max_escaped_char_length: Cell<u8>,
    pub error_code: Cell<ErrCode>,
    pub default_fetch_type: Cell<u16>,
    pub persistent_id: RefCell<Option<PhpStr>>,
    pub refcount: Cell<u32>,
    /// `def_stmt_ce` (`None` is `PDOStatement`).
    pub def_stmt_ce: RefCell<Option<ClassRef>>,
    pub def_stmt_ctor_args: RefCell<Option<PhpArrayRef>>,
    /// The statement of a failed `PDO::query()` (error reporting).
    pub query_stmt: RefCell<Option<(Rc<Stmt>, ObjRef)>>,
    /// `zend_replace_error_handling(EH_THROW, PDOException)` is active.
    pub in_ctor: Cell<bool>,
}

impl Default for Dbh {
    fn default() -> Self {
        Dbh::new()
    }
}

impl Dbh {
    pub fn new() -> Dbh {
        Dbh {
            methods: RefCell::new(None),
            driver: Cell::new(None),
            data_source: RefCell::new(PhpStr::new()),
            username: RefCell::new(None),
            password: RefCell::new(None),
            is_persistent: Cell::new(false),
            auto_commit: Cell::new(false),
            is_closed: Cell::new(false),
            alloc_own_columns: Cell::new(false),
            in_txn: Cell::new(false),
            stringify: Cell::new(false),
            skip_param_evt: Cell::new(0),
            error_mode: Cell::new(0),
            oracle_nulls: Cell::new(0),
            native_case: Cell::new(0),
            desired_case: Cell::new(0),
            max_escaped_char_length: Cell::new(0),
            error_code: Cell::new([0; 5]),
            default_fetch_type: Cell::new(0),
            persistent_id: RefCell::new(None),
            refcount: Cell::new(0),
            def_stmt_ce: RefCell::new(None),
            def_stmt_ctor_args: RefCell::new(None),
            query_stmt: RefCell::new(None),
            in_ctor: Cell::new(false),
        }
    }

    /// The driver operations.
    pub fn methods(&self) -> Option<Rc<dyn DbhMethods>> {
        self.methods.borrow().clone()
    }

    /// `PDO_DBH_CLEAR_ERR()`.
    pub fn clear_err(&self) {
        self.error_code.set(ERR_NONE);
        self.query_stmt.borrow_mut().take();
    }

    /// `pdo_is_in_transaction`.
    pub fn in_transaction(&self) -> bool {
        match self.methods().and_then(|m| m.in_transaction(self)) {
            Some(b) => b,
            None => self.in_txn.get(),
        }
    }
}

/// Native state of `PDO` objects (`pdo_dbh_object_t::inner`).
pub struct PdoHandle(pub Rc<Dbh>);

/// The handle of a `PDO` object.
pub fn dbh_of(obj: &ObjRef) -> Option<Rc<Dbh>> {
    obj.native::<PdoHandle>().map(|h| h.0.clone())
}

/// `pdo_dbh_new`.
pub fn create_pdo(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(PdoHandle(Rc::new(Dbh::new())));
    Ok(obj)
}

/// `Z_PDO_DBH_P(ZEND_THIS)` followed by `PDO_CONSTRUCT_CHECK`.
pub fn this_dbh(_vm: &mut Vm, args: &Args) -> PResult<(ObjRef, Rc<Dbh>)> {
    let this = args.this().cloned().expect("PDO method without $this");
    let dbh = dbh_of(&this).unwrap_or_else(|| {
        let d = Rc::new(Dbh::new());
        this.set_native(PdoHandle(d.clone()));
        d
    });
    Ok((this, dbh))
}

/// `PDO_CONSTRUCT_CHECK`.
pub fn construct_check(vm: &mut Vm, this: &ObjRef, dbh: &Dbh) -> PResult<()> {
    if dbh.driver.get().is_none() {
        return Err(vm.throw_error(
            "Error",
            format!("{} object is uninitialized", this.class().name),
        ));
    }
    Ok(())
}

/// The handle of `$this` after `PDO_CONSTRUCT_CHECK` (for driver methods).
pub fn checked_dbh(vm: &mut Vm, args: &Args) -> PResult<(ObjRef, Rc<Dbh>)> {
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    Ok((this, dbh))
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

/// `pdo_attr_lval`: an integer driver option.
pub fn attr_lval(options: Option<&PhpArrayRef>, attr: i64, default: i64) -> i64 {
    match options.and_then(|o| o.get_int(attr)) {
        Some(v) => to_int_silent(&v.unref()),
        None => default,
    }
}

/// `pdo_attr_strval`: a string driver option.
pub fn attr_strval(
    vm: &mut Vm,
    options: Option<&PhpArrayRef>,
    attr: i64,
    default: Option<PhpStr>,
) -> PResult<Option<PhpStr>> {
    match options.and_then(|o| o.get_int(attr)) {
        Some(v) => {
            let v = v.unref();
            Ok(Some(vm.to_php_string(&v)?))
        }
        None => Ok(default),
    }
}

/// `pdo_get_long_param`.
pub fn get_long_param(vm: &mut Vm, value: &Value) -> PResult<i64> {
    match value.unref() {
        v @ (Value::Int(_) | Value::Bool(_)) => Ok(to_int_silent(&v)),
        Value::String(s) if matches!(is_numeric_str(&s), Some(Numeric::Int(_))) => {
            match is_numeric_str(&s) {
                Some(Numeric::Int(i)) => Ok(i),
                _ => unreachable!(),
            }
        }
        other => Err(vm.throw_type_error(format!(
            "Attribute value must be of type int for selected attribute, {} given",
            other.value_name()
        ))),
    }
}

/// `pdo_get_bool_param`.
pub fn get_bool_param(vm: &mut Vm, value: &Value) -> PResult<bool> {
    match value.unref() {
        Value::Bool(b) => Ok(b),
        Value::Int(i) => Ok(i != 0),
        other => Err(vm.throw_type_error(format!(
            "Attribute value must be of type bool for selected attribute, {} given",
            other.value_name()
        ))),
    }
}

/// Validates an `ATTR_STATEMENT_CLASS` value; `arg` is the argument number
/// for the error messages (`None` in `PDO::prepare()` options).
fn statement_class_value(
    vm: &mut Vm,
    value: &Value,
    arg: Option<usize>,
) -> PResult<(ClassRef, Option<PhpArrayRef>)> {
    let fail_type = |vm: &mut Vm, msg: String| match arg {
        Some(n) => vm.argument_type_error(n, &msg),
        None => vm.throw_type_error(msg),
    };
    let Value::Array(arr) = value.unref() else {
        let msg = format!(
            "PDO::ATTR_STATEMENT_CLASS value must be of type array, {} given",
            value.unref().value_name()
        );
        return Err(fail_type(vm, msg));
    };
    let Some(item) = arr.get_int(0).map(Value::unref) else {
        let msg = "PDO::ATTR_STATEMENT_CLASS value must be an array with the format array(classname, constructor_args)";
        return Err(match arg {
            Some(n) => vm.argument_value_error(n, msg),
            None => vm.throw_value_error(msg),
        });
    };
    let ce = match &item {
        Value::String(name) => vm.lookup_class(name)?,
        _ => None,
    };
    let Some(ce) = ce else {
        return Err(fail_type(
            vm,
            "PDO::ATTR_STATEMENT_CLASS class must be a valid class".to_string(),
        ));
    };
    if !ce.instance_of_name(STMT_CLASS.as_bytes()) {
        return Err(fail_type(
            vm,
            "PDO::ATTR_STATEMENT_CLASS class must be derived from PDOStatement".to_string(),
        ));
    }
    if let Some(ctor) = &ce.magic.constructor {
        if ctor.flags & (member_flags::PRIVATE | member_flags::PROTECTED) == 0 {
            return Err(fail_type(
                vm,
                "User-supplied statement class cannot have a public constructor".to_string(),
            ));
        }
    }
    let ctor_args = match arr.get_int(1).map(Value::unref) {
        None => None,
        Some(Value::Array(a)) => Some(a),
        Some(other) => {
            let msg = format!(
                "PDO::ATTR_STATEMENT_CLASS constructor_args must be of type ?array, {} given",
                other.value_name()
            );
            return Err(fail_type(vm, msg));
        }
    };
    Ok((ce, ctor_args))
}

/// `pdo_dbh_attribute_set`.
fn attribute_set(
    vm: &mut Vm,
    dbh: &Dbh,
    attr: i64,
    value: &Value,
    arg_num: usize,
) -> PResult<bool> {
    match attr {
        ATTR_ERRMODE => {
            let l = get_long_param(vm, value)?;
            match l {
                0..=2 => {
                    dbh.error_mode.set(l as u8);
                    return Ok(true);
                }
                _ => {
                    return Err(vm.argument_value_error(
                        arg_num,
                        "Error mode must be one of the PDO::ERRMODE_* constants",
                    ));
                }
            }
        }
        ATTR_CASE => {
            let l = get_long_param(vm, value)?;
            match l {
                0..=2 => {
                    dbh.desired_case.set(l as u8);
                    return Ok(true);
                }
                _ => {
                    return Err(vm.argument_value_error(
                        arg_num,
                        "Case folding mode must be one of the PDO::CASE_* constants",
                    ));
                }
            }
        }
        ATTR_ORACLE_NULLS => {
            let l = get_long_param(vm, value)?;
            match l {
                0..=2 => {
                    dbh.oracle_nulls.set(l as u8);
                    return Ok(true);
                }
                _ => {
                    return Err(vm.argument_value_error(
                        arg_num,
                        "PDO::ATTR_ORACLE_NULLS must be one of the PDO::NULL_* constants",
                    ));
                }
            }
        }
        ATTR_DEFAULT_FETCH_MODE => {
            let l = match value.unref() {
                Value::Array(a) => {
                    if let Some(Value::Int(m)) = a.get_int(0).map(Value::unref) {
                        if m == FETCH_INTO || m == FETCH_CLASS {
                            return Err(vm.argument_value_error(
                                arg_num,
                                "PDO::FETCH_INTO and PDO::FETCH_CLASS cannot be set as the default fetch mode",
                            ));
                        }
                    }
                    to_int_silent(&Value::Array(a))
                }
                other => get_long_param(vm, &other)?,
            };
            if l == FETCH_USE_DEFAULT {
                return Err(vm.argument_value_error(
                    arg_num,
                    "Fetch mode must be a bitmask of PDO::FETCH_* constants",
                ));
            }
            dbh.default_fetch_type.set(l as u16);
            return Ok(true);
        }
        ATTR_STRINGIFY_FETCHES => {
            let b = get_bool_param(vm, value)?;
            dbh.stringify.set(b);
            if let Some(m) = dbh.methods().filter(|m| m.supports_set_attribute()) {
                m.set_attribute(vm, dbh, attr, value)?;
            }
            return Ok(true);
        }
        ATTR_STATEMENT_CLASS => {
            if dbh.is_persistent.get() {
                raise_impl_error(
                    vm,
                    dbh,
                    None,
                    "HY000",
                    Some("PDO::ATTR_STATEMENT_CLASS cannot be used with persistent PDO instances"),
                )?;
                return Ok(false);
            }
            let (ce, ctor_args) = statement_class_value(vm, value, Some(arg_num))?;
            *dbh.def_stmt_ce.borrow_mut() = Some(ce);
            *dbh.def_stmt_ctor_args.borrow_mut() = ctor_args;
            return Ok(true);
        }
        // Not a ValueError: the attribute might be a driver-specific one.
        _ => {}
    }
    let methods = dbh.methods().filter(|m| m.supports_set_attribute());
    if let Some(m) = &methods {
        dbh.clear_err();
        if m.set_attribute(vm, dbh, attr, value)? {
            return Ok(true);
        }
    }
    if methods.is_none() {
        raise_impl_error(
            vm,
            dbh,
            None,
            "IM001",
            Some("driver does not support setting attributes"),
        )?;
    } else {
        handle_dbh_err(vm, dbh)?;
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

fn find_driver(name: &[u8]) -> Option<&'static dyn Driver> {
    DRIVERS
        .iter()
        .copied()
        .find(|d| d.name().as_bytes() == name)
}

/// `create_driver_specific_pdo_object`: picks (and for `connect()`
/// instantiates) the class for `driver`.
fn driver_specific_object(
    vm: &mut Vm,
    driver: &'static dyn Driver,
    called_scope: &ClassRef,
    connect: bool,
) -> PResult<Option<ObjRef>> {
    let method = if connect { "connect" } else { "__construct" };
    let is_pdo = called_scope.lc_name.as_bytes() == b"pdo";
    let by_driver = driver
        .specific_class()
        .and_then(|n| vm.find_class(n.as_bytes()));
    let mut by_called: Option<ClassRef> = None;
    for d in DRIVERS {
        if let Some(ce) = d.specific_class().and_then(|n| vm.find_class(n.as_bytes())) {
            if !is_pdo && called_scope.instance_of(&ce) {
                by_called = Some(called_scope.clone());
                break;
            }
        }
    }
    if let Some(called) = by_called {
        if let Some(by_driver) = &by_driver {
            if !called.instance_of(by_driver) {
                return Err(error::throw(
                    vm,
                    format!(
                        "{}::{method}() cannot be used for connecting to the \"{}\" driver, either call {}::{method}() or PDO::{method}() instead",
                        called_scope.name,
                        driver.name(),
                        by_driver.name
                    ),
                ));
            }
            return Ok(if connect {
                Some(vm.instantiate(called_scope)?)
            } else {
                None
            });
        }
        return Err(error::throw(
            vm,
            format!(
                "{}::{method}() cannot be used for connecting to an unknown driver, call PDO::{method}() instead",
                called_scope.name
            ),
        ));
    }
    // A non-driver specific PDO subclass instantiated via the constructor:
    // the legacy behaviour.
    if !is_pdo && !connect {
        return Ok(None);
    }
    if let Some(by_driver) = by_driver {
        if !is_pdo {
            return Err(error::throw(
                vm,
                format!(
                    "{}::{method}() cannot be used for connecting to the \"{}\" driver, either call {}::{method}() or PDO::{method}() instead",
                    called_scope.name,
                    driver.name(),
                    by_driver.name
                ),
            ));
        }
        if connect {
            return Ok(Some(vm.instantiate(&by_driver)?));
        }
    } else if connect {
        return Ok(Some(vm.instantiate(called_scope)?));
    }
    Ok(None)
}

/// `dsn_from_uri`: the first line of the resource at `uri`.
fn dsn_from_uri(vm: &mut Vm, uri: &[u8]) -> PResult<Option<Vec<u8>>> {
    let Some(res) = crate::standard::streams::open(vm, uri, b"rb", false, None)? else {
        return Ok(None);
    };
    let line = crate::standard::streams::read_line(vm, &res, Some(512))?;
    crate::standard::streams::close(vm, &res)?;
    Ok(line)
}

/// `php_pdo_internal_construct_driver`: `PDO::__construct()` (with
/// `current`) and `PDO::connect()` (returns the new object).
fn internal_construct(
    vm: &mut Vm,
    args: &Args,
    current: Option<&ObjRef>,
    called_scope: &ClassRef,
) -> PResult<Option<ObjRef>> {
    let connect = current.is_none();
    let raw = args.str(0);
    let mut dsn: Vec<u8> = raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())].to_vec();
    let username = args.str_opt(1).cloned();
    let password = args.str_opt(2).cloned();
    let options = args.array_opt(3).cloned();

    let mut colon = dsn.iter().position(|&c| c == b':');
    if colon.is_none() {
        // A DSN alias from php.ini (`pdo.dsn.<name>`).
        let alt = format!("pdo.dsn.{}", String::from_utf8_lossy(&dsn));
        let Some(ini_dsn) = vm.ini_get(&alt) else {
            return Err(vm.argument_error(
                error::EXCEPTION_CLASS,
                1,
                "must be a valid data source name",
            ));
        };
        dsn = ini_dsn.to_vec();
        colon = dsn.iter().position(|&c| c == b':');
        if colon.is_none() {
            return Err(error::throw(
                vm,
                format!("invalid data source name (via INI: {alt})"),
            ));
        }
    }
    if dsn.starts_with(b"uri:") {
        vm.engine_deprecated(
            "Looking up the DSN from a URI is deprecated due to possible security concerns with DSNs coming from remote URIs",
        )?;
        let uri = dsn[4..].to_vec();
        let Some(line) = dsn_from_uri(vm, &uri)? else {
            return Err(vm.argument_error(
                error::EXCEPTION_CLASS,
                1,
                "must be a valid data source URI",
            ));
        };
        dsn = line[..line.iter().position(|&c| c == 0).unwrap_or(line.len())].to_vec();
        colon = dsn.iter().position(|&c| c == b':');
        if colon.is_none() {
            return Err(vm.argument_error(
                error::EXCEPTION_CLASS,
                1,
                "must be a valid data source name (via URI)",
            ));
        }
    }
    let colon = colon.expect("checked above");

    let Some(driver) = find_driver(&dsn[..colon]) else {
        // The DSN is not shown: it might contain a password.
        return Err(error::throw(vm, "could not find driver"));
    };

    let new_obj = driver_specific_object(vm, driver, called_scope, connect)?;
    let object = match (&new_obj, current) {
        (Some(o), _) => o.clone(),
        (None, Some(c)) => c.clone(),
        (None, None) => unreachable!("connect() always creates an object"),
    };
    let mut dbh = match dbh_of(&object) {
        Some(d) => d,
        None => {
            let d = Rc::new(Dbh::new());
            object.set_native(PdoHandle(d.clone()));
            d
        }
    };

    // Persistent connections.
    let mut is_persistent = false;
    let mut call_factory = true;
    if let Some(opts) = &options {
        let user = username.clone().unwrap_or_default();
        let pass = password.clone().unwrap_or_default();
        let mut hash_key: Option<Vec<u8>> = None;
        if let Some(v) = opts.get_int(ATTR_PERSISTENT).map(Value::unref) {
            let mut key = b"PDO:DBH:DSN=".to_vec();
            key.extend_from_slice(&dsn);
            key.push(b':');
            key.extend_from_slice(&user);
            key.push(b':');
            key.extend_from_slice(&pass);
            match &v {
                Value::String(s) if is_numeric_str(s).is_none() && !s.is_empty() => {
                    key.push(b':');
                    key.extend_from_slice(s);
                    is_persistent = true;
                }
                other => is_persistent = to_int_silent(other) != 0,
            }
            hash_key = Some(key);
        }
        if is_persistent {
            let key = hash_key.expect("set with is_persistent");
            let cached = vm.ext_data::<PdoGlobals>().persistent.get(&key).cloned();
            let pdbh = match cached {
                Some(p) if p.methods().is_none_or(|m| m.check_liveness(&p)) => {
                    call_factory = false;
                    p
                }
                stale => {
                    if stale.is_some() {
                        vm.ext_data::<PdoGlobals>().persistent.remove(&key);
                    }
                    let p = Rc::new(Dbh::new());
                    p.refcount.set(1);
                    p.is_persistent.set(true);
                    *p.persistent_id.borrow_mut() = Some(PhpStr::from_vec(key.clone()));
                    *p.def_stmt_ce.borrow_mut() = dbh.def_stmt_ce.borrow().clone();
                    p
                }
            };
            object.set_native(PdoHandle(pdbh.clone()));
            pdbh.refcount.set(pdbh.refcount.get() + 1);
            dbh = pdbh;
        }
    }

    if call_factory {
        *dbh.data_source.borrow_mut() = PhpStr::from_bytes(&dsn[colon + 1..]);
        *dbh.username.borrow_mut() = username.clone();
        *dbh.password.borrow_mut() = password.clone();
        dbh.default_fetch_type.set(FETCH_BOTH as u16);
    }
    dbh.auto_commit
        .set(attr_lval(options.as_ref(), ATTR_AUTOCOMMIT, 1) != 0);
    dbh.error_mode
        .set(attr_lval(options.as_ref(), ATTR_ERRMODE, ERRMODE_EXCEPTION as i64) as u8);

    dbh.in_ctor.set(true);
    let r = construct_finish(
        vm,
        driver,
        &dbh,
        options.as_ref(),
        call_factory,
        is_persistent,
    );
    dbh.in_ctor.set(false);
    r?;
    Ok(new_obj)
}

fn construct_finish(
    vm: &mut Vm,
    driver: &'static dyn Driver,
    dbh: &Rc<Dbh>,
    options: Option<&PhpArrayRef>,
    call_factory: bool,
    is_persistent: bool,
) -> PResult<()> {
    if call_factory {
        if !driver.handle_factory(vm, dbh, options)? {
            if is_persistent {
                dbh.refcount.set(dbh.refcount.get().saturating_sub(1));
            }
            return Err(error::throw(vm, "Constructor failed"));
        }
        if is_persistent {
            let key = dbh
                .persistent_id
                .borrow()
                .clone()
                .unwrap_or_default()
                .to_vec();
            vm.ext_data::<PdoGlobals>()
                .persistent
                .insert(key, dbh.clone());
        }
        dbh.driver.set(Some(driver));
    }
    let mut pending: Option<Flow> = None;
    if let Some(opts) = options {
        for (k, v) in opts.iter() {
            let ArrayKey::Int(attr) = k else {
                continue;
            };
            // php-src keeps going with an exception pending; later ones chain.
            if let Err(e) = attribute_set(vm, dbh, attr, &v.unref(), 4) {
                pending = Some(match pending {
                    Some(p) => error::chain(e, p),
                    None => e,
                });
            }
        }
    }
    match pending {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

macro_rules! method {
    ($name:ident, $imp:ident) => {
        fn $name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            let dbh = args.this().and_then(dbh_of);
            let r = $imp(vm, args);
            finish(vm, dbh.as_ref(), r)
        }
    };
}

fn construct_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = args
        .this()
        .cloned()
        .expect("PDO::__construct without $this");
    let ce = this.class().clone();
    internal_construct(vm, args, Some(&this), &ce)?;
    Ok(Value::Null)
}
method!(construct, construct_impl);

fn connect_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let scope = match args.scope().cloned() {
        Some(s) => s,
        None => super::find_class(vm, PDO_CLASS),
    };
    let obj = internal_construct(vm, args, None, &scope)?;
    Ok(obj.map_or(Value::Null, Value::Object))
}

fn connect(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    connect_impl(vm, args)
}

/// The statement class and constructor arguments for a new statement.
fn statement_class(vm: &mut Vm, dbh: &Dbh) -> (ClassRef, Option<PhpArrayRef>) {
    let ce = dbh
        .def_stmt_ce
        .borrow()
        .clone()
        .unwrap_or_else(|| super::find_class(vm, STMT_CLASS));
    (ce, dbh.def_stmt_ctor_args.borrow().clone())
}

/// Wires a new statement to its handle (`stmt->dbh`, `database_object_handle`).
fn attach_statement(stmt: &Stmt, dbh: &Rc<Dbh>, dbh_obj: &ObjRef, query: &PhpStr) {
    *stmt.query_string.borrow_mut() = Some(query.clone());
    stmt.default_fetch_type
        .set(dbh.default_fetch_type.get() as i64);
    *stmt.dbh.borrow_mut() = Some(dbh.clone());
    *stmt.database_object_handle.borrow_mut() = Some(dbh_obj.clone());
}

fn prepare_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let query = args.str(0).clone();
    let options = args.array(1).clone();
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    if query.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    dbh.clear_err();
    let (ce, ctor_args) = match options.get_int(ATTR_STATEMENT_CLASS) {
        Some(value) => {
            let value = value.clone();
            statement_class_value(vm, &value, None)?
        }
        None => statement_class(vm, &dbh),
    };
    let obj = stmt::instantiate(vm, &ce, ctor_args.as_ref())?;
    let stmt = stmt::stmt_of(&obj).expect("statements carry native state");
    attach_statement(&stmt, &dbh, &this, &query);
    let methods = dbh.methods().expect("constructed handles have methods");
    if methods.prepare(vm, &dbh, &query, &stmt, Some(&options))? {
        stmt::construct(vm, &obj, &stmt, ctor_args.as_ref())?;
        return Ok(Value::Object(obj));
    }
    handle_dbh_err(vm, &dbh)?;
    Ok(Value::Bool(false))
}
method!(prepare, prepare_impl);

fn begin_transaction_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    if dbh.in_transaction() {
        return Err(error::throw(vm, "There is already an active transaction"));
    }
    let Some(m) = dbh.methods().filter(|m| m.supports_transactions()) else {
        return Err(error::throw(vm, "This driver doesn't support transactions"));
    };
    if m.begin(vm, &dbh)? {
        dbh.in_txn.set(true);
        return Ok(Value::Bool(true));
    }
    handle_dbh_err(vm, &dbh)?;
    Ok(Value::Bool(false))
}
method!(begin_transaction, begin_transaction_impl);

fn commit_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    if !dbh.in_transaction() {
        return Err(error::throw(vm, "There is no active transaction"));
    }
    let m = dbh.methods().expect("constructed handles have methods");
    if m.commit(vm, &dbh)? {
        dbh.in_txn.set(false);
        return Ok(Value::Bool(true));
    }
    handle_dbh_err(vm, &dbh)?;
    Ok(Value::Bool(false))
}
method!(commit, commit_impl);

fn roll_back_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    if !dbh.in_transaction() {
        return Err(error::throw(vm, "There is no active transaction"));
    }
    let m = dbh.methods().expect("constructed handles have methods");
    if m.rollback(vm, &dbh)? {
        dbh.in_txn.set(false);
        return Ok(Value::Bool(true));
    }
    handle_dbh_err(vm, &dbh)?;
    Ok(Value::Bool(false))
}
method!(roll_back, roll_back_impl);

fn in_transaction_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    Ok(Value::Bool(dbh.in_transaction()))
}
method!(in_transaction, in_transaction_impl);

fn set_attribute_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let attr = args.int(0);
    let value = args.get(1).clone();
    let (this, dbh) = this_dbh(vm, args)?;
    dbh.clear_err();
    construct_check(vm, &this, &dbh)?;
    Ok(Value::Bool(attribute_set(vm, &dbh, attr, &value, 2)?))
}
method!(set_attribute, set_attribute_impl);

fn get_attribute_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let attr = args.int(0);
    let (this, dbh) = this_dbh(vm, args)?;
    dbh.clear_err();
    construct_check(vm, &this, &dbh)?;
    match attr {
        ATTR_PERSISTENT => return Ok(Value::Bool(dbh.is_persistent.get())),
        ATTR_CASE => return Ok(Value::Int(dbh.desired_case.get() as i64)),
        ATTR_ORACLE_NULLS => return Ok(Value::Int(dbh.oracle_nulls.get() as i64)),
        ATTR_ERRMODE => return Ok(Value::Int(dbh.error_mode.get() as i64)),
        ATTR_DRIVER_NAME => {
            let name = dbh.driver.get().map_or("", |d| d.name());
            return Ok(Value::from(name));
        }
        ATTR_STATEMENT_CLASS => {
            let (ce, args) = statement_class(vm, &dbh);
            let mut arr = PhpArray::new();
            let _ = arr.append(Value::String(ce.name.clone()));
            if let Some(a) = args {
                let _ = arr.append(Value::Array(a));
            }
            return Ok(Value::array(arr));
        }
        ATTR_DEFAULT_FETCH_MODE => return Ok(Value::Int(dbh.default_fetch_type.get() as i64)),
        ATTR_STRINGIFY_FETCHES => return Ok(Value::Bool(dbh.stringify.get())),
        _ => {}
    }
    let Some(m) = dbh.methods().filter(|m| m.supports_get_attribute()) else {
        raise_impl_error(
            vm,
            &dbh,
            None,
            "IM001",
            Some("driver does not support getting attributes"),
        )?;
        return Ok(Value::Bool(false));
    };
    match m.get_attribute(vm, &dbh, attr)? {
        AttrValue::Error => {
            handle_dbh_err(vm, &dbh)?;
            Ok(Value::Bool(false))
        }
        AttrValue::Unsupported => {
            raise_impl_error(
                vm,
                &dbh,
                None,
                "IM001",
                Some("driver does not support that attribute"),
            )?;
            Ok(Value::Bool(false))
        }
        AttrValue::Value(v) => Ok(v),
    }
}
method!(get_attribute, get_attribute_impl);

fn exec_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let sql = args.str(0).clone();
    if sql.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    let (this, dbh) = this_dbh(vm, args)?;
    dbh.clear_err();
    construct_check(vm, &this, &dbh)?;
    let m = dbh.methods().expect("constructed handles have methods");
    let ret = m.exec(vm, &dbh, &sql)?;
    if ret == -1 {
        handle_dbh_err(vm, &dbh)?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Int(ret))
}
method!(exec, exec_impl);

fn last_insert_id_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str_opt(0).cloned();
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    dbh.clear_err();
    let Some(m) = dbh.methods().filter(|m| m.supports_last_id()) else {
        raise_impl_error(
            vm,
            &dbh,
            None,
            "IM001",
            Some("driver does not support lastInsertId()"),
        )?;
        return Ok(Value::Bool(false));
    };
    match m.last_insert_id(vm, &dbh, name.as_ref())? {
        Some(id) => Ok(Value::String(id)),
        None => {
            handle_dbh_err(vm, &dbh)?;
            Ok(Value::Bool(false))
        }
    }
}
method!(last_insert_id, last_insert_id_impl);

fn error_code_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    if let Some((stmt, _)) = &*dbh.query_stmt.borrow() {
        return Ok(Value::String(err_str(&stmt.error_code.get())));
    }
    let code = dbh.error_code.get();
    if code[0] == 0 {
        return Ok(Value::Null);
    }
    Ok(Value::String(err_str(&code)))
}
method!(error_code, error_code_impl);

fn error_info_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    let mut arr = PhpArray::new();
    let query_stmt = dbh.query_stmt.borrow().clone();
    let code = match &query_stmt {
        Some((stmt, _)) => stmt.error_code.get(),
        None => dbh.error_code.get(),
    };
    let _ = arr.append(Value::String(err_str(&code)));
    if err_str(&code).as_bytes() != b"00000" {
        if let Some(m) = dbh.methods().filter(|m| m.supports_fetch_err()) {
            m.fetch_err(&dbh, query_stmt.as_ref().map(|(s, _)| &**s), &mut arr);
        }
    }
    while arr.len() < 3 {
        let _ = arr.append(Value::Null);
    }
    Ok(Value::array(arr))
}
method!(error_info, error_info_impl);

fn query_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let query = args.str(0).clone();
    let fetch_mode = args.int_opt(1);
    let mode_args: Vec<Value> = args.rest(2).to_vec();
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    if query.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    dbh.clear_err();
    let (ce, ctor_args) = statement_class(vm, &dbh);
    let obj = stmt::instantiate(vm, &ce, ctor_args.as_ref())?;
    let stmt = stmt::stmt_of(&obj).expect("statements carry native state");
    attach_statement(&stmt, &dbh, &this, &query);
    *stmt.active_query_string.borrow_mut() = ActiveQuery::SameAsQuery;
    let methods = dbh.methods().expect("constructed handles have methods");
    if !methods.prepare(vm, &dbh, &query, &stmt, None)? {
        handle_dbh_err(vm, &dbh)?;
        return Ok(Value::Bool(false));
    }
    stmt.clear_err();
    let mode_ok = match fetch_mode {
        None => true,
        Some(mode) => {
            stmt::setup_fetch_mode(vm, &stmt, &dbh, mode, 2, &mode_args)?;
            true
        }
    };
    if mode_ok {
        stmt.clear_err();
        if stmt.with_driver(vm, |d, vm| d.execute(vm, &stmt))? {
            let mut ok = true;
            if !stmt.executed.get() {
                if dbh.alloc_own_columns.get() {
                    ok = stmt::describe_columns(vm, &stmt, &dbh)?;
                }
                stmt.executed.set(true);
            }
            if ok {
                stmt::construct(vm, &obj, &stmt, ctor_args.as_ref())?;
                return Ok(Value::Object(obj));
            }
        }
    }
    // Something broke: keep the statement for errorCode()/errorInfo().
    *dbh.query_stmt.borrow_mut() = Some((stmt.clone(), obj.clone()));
    stmt.database_object_handle.borrow_mut().take();
    handle_stmt_err(vm, &dbh, &stmt)?;
    Ok(Value::Bool(false))
}
method!(query, query_impl);

fn quote_impl(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let param_type = args.int(1);
    let (this, dbh) = this_dbh(vm, args)?;
    construct_check(vm, &this, &dbh)?;
    dbh.clear_err();
    let Some(m) = dbh.methods().filter(|m| m.supports_quote()) else {
        raise_impl_error(
            vm,
            &dbh,
            None,
            "IM001",
            Some("driver does not support quoting"),
        )?;
        return Ok(Value::Bool(false));
    };
    match m.quote(vm, &dbh, &s, param_type)? {
        Some(q) => Ok(Value::String(q)),
        None => {
            handle_dbh_err(vm, &dbh)?;
            Ok(Value::Bool(false))
        }
    }
}
method!(quote, quote_impl);

/// `PDO::getAvailableDrivers()` / `pdo_drivers()`.
pub fn available_drivers(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let mut arr = PhpArray::new();
    for d in DRIVERS {
        let _ = arr.append(Value::from(d.name()));
    }
    Ok(Value::array(arr))
}

/// `pdo_dbh_free_storage`: rolls back an open transaction when the object
/// goes away (the driver connection itself closes when its last user drops).
fn dtor(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    let Some(dbh) = dbh_of(obj) else {
        return Ok(());
    };
    let Some(m) = dbh.methods() else {
        return Ok(());
    };
    if dbh.driver.get().is_some()
        && m.supports_transactions()
        && (!dbh.is_persistent.get() || dbh.refcount.get() <= 2)
        && dbh.in_transaction()
    {
        m.rollback(vm, &dbh)?;
        dbh.in_txn.set(false);
    }
    if dbh.is_persistent.get() {
        m.persistent_shutdown(&dbh);
        dbh.refcount.set(dbh.refcount.get().saturating_sub(1));
    } else {
        m.close(vm, &dbh);
    }
    Ok(())
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
    CONSTRUCT: "public function __construct(string $dsn, ?string $username = null, #[\\SensitiveParameter] ?string $password = null, ?array $options = null)" => construct;
    CONNECT: "public static function connect(string $dsn, ?string $username = null, #[\\SensitiveParameter] ?string $password = null, ?array $options = null): static" => connect;
    BEGIN_TRANSACTION: "public function beginTransaction(): bool" => begin_transaction;
    COMMIT: "public function commit(): bool" => commit;
    ERROR_CODE: "public function errorCode(): ?string" => error_code;
    ERROR_INFO: "public function errorInfo(): array" => error_info;
    EXEC: "public function exec(string $statement): int|false" => exec;
    GET_ATTRIBUTE: "public function getAttribute(int $attribute): mixed" => get_attribute;
    GET_AVAILABLE_DRIVERS: "public static function getAvailableDrivers(): array" => available_drivers;
    IN_TRANSACTION: "public function inTransaction(): bool" => in_transaction;
    LAST_INSERT_ID: "public function lastInsertId(?string $name = null): string|false" => last_insert_id;
    PREPARE: "public function prepare(string $query, array $options = []): PDOStatement|false" => prepare;
    QUERY: "public function query(string $query, ?int $fetchMode = null, mixed ...$fetchModeArgs): PDOStatement|false" => query;
    QUOTE: "public function quote(string $string, int $type = PDO::PARAM_STR): string|false" => quote;
    ROLL_BACK: "public function rollBack(): bool" => roll_back;
    SET_ATTRIBUTE: "public function setAttribute(int $attribute, mixed $value): bool" => set_attribute;
}

/// `PDO` class constants in stub order.
const CONSTANTS: &[(&str, i64)] = &[
    ("PARAM_NULL", PARAM_NULL),
    ("PARAM_BOOL", PARAM_BOOL),
    ("PARAM_INT", PARAM_INT),
    ("PARAM_STR", PARAM_STR),
    ("PARAM_LOB", PARAM_LOB),
    ("PARAM_STMT", PARAM_STMT),
    ("PARAM_INPUT_OUTPUT", PARAM_INPUT_OUTPUT),
    ("PARAM_STR_NATL", PARAM_STR_NATL),
    ("PARAM_STR_CHAR", PARAM_STR_CHAR),
    ("PARAM_EVT_ALLOC", 0),
    ("PARAM_EVT_FREE", 1),
    ("PARAM_EVT_EXEC_PRE", 2),
    ("PARAM_EVT_EXEC_POST", 3),
    ("PARAM_EVT_FETCH_PRE", 4),
    ("PARAM_EVT_FETCH_POST", 5),
    ("PARAM_EVT_NORMALIZE", 6),
    ("FETCH_DEFAULT", FETCH_USE_DEFAULT),
    ("FETCH_LAZY", FETCH_LAZY),
    ("FETCH_ASSOC", FETCH_ASSOC),
    ("FETCH_NUM", FETCH_NUM),
    ("FETCH_BOTH", FETCH_BOTH),
    ("FETCH_OBJ", FETCH_OBJ),
    ("FETCH_BOUND", FETCH_BOUND),
    ("FETCH_COLUMN", FETCH_COLUMN),
    ("FETCH_CLASS", FETCH_CLASS),
    ("FETCH_INTO", FETCH_INTO),
    ("FETCH_FUNC", FETCH_FUNC),
    ("FETCH_GROUP", FETCH_GROUP),
    ("FETCH_UNIQUE", FETCH_UNIQUE),
    ("FETCH_KEY_PAIR", FETCH_KEY_PAIR),
    ("FETCH_CLASSTYPE", FETCH_CLASSTYPE),
    ("FETCH_SERIALIZE", FETCH_SERIALIZE),
    ("FETCH_PROPS_LATE", FETCH_PROPS_LATE),
    ("FETCH_NAMED", FETCH_NAMED),
    ("ATTR_AUTOCOMMIT", ATTR_AUTOCOMMIT),
    ("ATTR_PREFETCH", ATTR_PREFETCH),
    ("ATTR_TIMEOUT", ATTR_TIMEOUT),
    ("ATTR_ERRMODE", ATTR_ERRMODE),
    ("ATTR_SERVER_VERSION", ATTR_SERVER_VERSION),
    ("ATTR_CLIENT_VERSION", ATTR_CLIENT_VERSION),
    ("ATTR_SERVER_INFO", ATTR_SERVER_INFO),
    ("ATTR_CONNECTION_STATUS", ATTR_CONNECTION_STATUS),
    ("ATTR_CASE", ATTR_CASE),
    ("ATTR_CURSOR_NAME", ATTR_CURSOR_NAME),
    ("ATTR_CURSOR", ATTR_CURSOR),
    ("ATTR_ORACLE_NULLS", ATTR_ORACLE_NULLS),
    ("ATTR_PERSISTENT", ATTR_PERSISTENT),
    ("ATTR_STATEMENT_CLASS", ATTR_STATEMENT_CLASS),
    ("ATTR_FETCH_TABLE_NAMES", ATTR_FETCH_TABLE_NAMES),
    ("ATTR_FETCH_CATALOG_NAMES", ATTR_FETCH_CATALOG_NAMES),
    ("ATTR_DRIVER_NAME", ATTR_DRIVER_NAME),
    ("ATTR_STRINGIFY_FETCHES", ATTR_STRINGIFY_FETCHES),
    ("ATTR_MAX_COLUMN_LEN", ATTR_MAX_COLUMN_LEN),
    ("ATTR_EMULATE_PREPARES", ATTR_EMULATE_PREPARES),
    ("ATTR_DEFAULT_FETCH_MODE", ATTR_DEFAULT_FETCH_MODE),
    ("ATTR_DEFAULT_STR_PARAM", ATTR_DEFAULT_STR_PARAM),
    ("ERRMODE_SILENT", ERRMODE_SILENT as i64),
    ("ERRMODE_WARNING", ERRMODE_WARNING as i64),
    ("ERRMODE_EXCEPTION", ERRMODE_EXCEPTION as i64),
    ("CASE_NATURAL", CASE_NATURAL as i64),
    ("CASE_LOWER", CASE_LOWER as i64),
    ("CASE_UPPER", CASE_UPPER as i64),
    ("NULL_NATURAL", NULL_NATURAL as i64),
    ("NULL_EMPTY_STRING", NULL_EMPTY_STRING as i64),
    ("NULL_TO_STRING", NULL_TO_STRING as i64),
];

/// Registers `PDO` with its constants, methods and the driver-specific
/// methods and deprecated constants of the bundled drivers.
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(PDO_CLASS, class_flags::NOT_SERIALIZABLE);
    for (name, value) in CONSTANTS {
        ce.add_constant(ClassConstant::new(*name, Value::Int(*value), PDO_CLASS));
        if *name == "NULL_TO_STRING" {
            ce.add_constant(ClassConstant::new(
                "ERR_NONE",
                Value::from("00000"),
                PDO_CLASS,
            ));
        }
    }
    for (name, value) in [
        ("FETCH_ORI_NEXT", FETCH_ORI_NEXT),
        ("FETCH_ORI_PRIOR", 1),
        ("FETCH_ORI_FIRST", 2),
        ("FETCH_ORI_LAST", 3),
        ("FETCH_ORI_ABS", 4),
        ("FETCH_ORI_REL", 5),
        ("CURSOR_FWDONLY", 0),
        ("CURSOR_SCROLL", 1),
    ] {
        ce.add_constant(ClassConstant::new(name, Value::Int(value), PDO_CLASS));
    }
    for nf in [
        &CONSTRUCT,
        &BEGIN_TRANSACTION,
        &COMMIT,
        &ERROR_CODE,
        &ERROR_INFO,
        &EXEC,
        &GET_ATTRIBUTE,
        &IN_TRANSACTION,
        &LAST_INSERT_ID,
        &PREPARE,
        &QUERY,
        &QUOTE,
        &ROLL_BACK,
        &SET_ATTRIBUTE,
    ] {
        ce.add_method(Function::native_method(nf, PDO_CLASS, member_flags::PUBLIC));
    }
    for nf in [&CONNECT, &GET_AVAILABLE_DRIVERS] {
        ce.add_method(Function::native_method(
            nf,
            PDO_CLASS,
            member_flags::PUBLIC | member_flags::STATIC,
        ));
    }
    for d in DRIVERS {
        for (name, value, message) in d.pdo_constants() {
            let mut c = ClassConstant::new(*name, Value::Int(*value), PDO_CLASS);
            c.flags |= member_flags::DEPRECATED;
            c.doc_comment = Some(PhpStr::from(format!("since 8.5, {message}")));
            ce.add_constant(c);
        }
        for nf in d.pdo_methods() {
            ce.add_method(Function::native_method(nf, PDO_CLASS, member_flags::PUBLIC));
        }
    }
    ce.handlers.create_object = Some(create_pdo);
    ce.handlers.clone_obj = Some(clone_forbidden);
    ce.handlers.dtor_obj = Some(dtor);
    r.class(ce);
}
