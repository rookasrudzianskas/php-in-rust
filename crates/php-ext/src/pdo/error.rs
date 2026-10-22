//! Error reporting (`pdo_raise_impl_error`, `pdo_handle_error`,
//! `pdo_throw_exception` from `pdo_dbh.c`) and the `PDOException` class.

use php_runtime::class::{ClassEntry, PropertyInfo, member_flags};
use php_runtime::types::TypeDecl;
use php_runtime::{Flow, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::dbh::{Dbh, ERRMODE_EXCEPTION, ERRMODE_SILENT, ERRMODE_WARNING};
use super::sqlstate::state_to_description;
use super::stmt::Stmt;

/// The `PDOException` class name.
pub const EXCEPTION_CLASS: &str = "PDOException";

/// A SQLSTATE (`pdo_error_type`); all-NUL is the never-set state.
pub type ErrCode = [u8; 5];

/// `PDO_ERR_NONE`.
pub const ERR_NONE: ErrCode = *b"00000";

/// The error code as PHP sees it (`char[6]`, NUL-terminated).
pub fn err_str(code: &ErrCode) -> PhpStr {
    let end = code.iter().position(|&c| c == 0).unwrap_or(code.len());
    PhpStr::from_bytes(&code[..end])
}

/// Five-byte SQLSTATE from a literal.
pub fn sqlstate(s: &str) -> ErrCode {
    let mut c = [0u8; 5];
    for (d, s) in c.iter_mut().zip(s.bytes()) {
        *d = s;
    }
    c
}

/// Creates (without throwing) a `PDOException` with `message` and `code`.
pub fn new_exception(vm: &mut Vm, message: impl Into<PhpStr>, code: Value) -> ObjRef {
    let obj = vm.create_exception(EXCEPTION_CLASS, message, 0);
    obj.set_prop(b"code", code);
    obj
}

/// `zend_throw_exception_ex(php_pdo_get_exception(), 0, msg)`.
pub fn throw(vm: &mut Vm, message: impl Into<PhpStr>) -> Flow {
    Flow::Exception(new_exception(vm, message, Value::Int(0)))
}

/// `pdo_throw_exception`: `SQLSTATE[%s] [%d] %s` with a full `errorInfo`.
pub fn throw_with_info(vm: &mut Vm, driver_code: i64, driver_msg: &str, state: &ErrCode) -> Flow {
    let st = err_str(state);
    let ex = new_exception(
        vm,
        format!("SQLSTATE[{st}] [{driver_code}] {driver_msg}"),
        Value::Int(driver_code),
    );
    let mut info = PhpArray::new();
    let _ = info.append(Value::String(st));
    let _ = info.append(Value::Int(driver_code));
    let _ = info.append(Value::from(driver_msg));
    ex.set_prop(b"errorInfo", Value::array(info));
    Flow::Exception(ex)
}

/// A warning raised by PDO; inside `PDO::__construct()` php-src replaces the
/// error handling with `EH_THROW` (warnings become `PDOException`s carrying
/// the full `"PDO::__construct(): ..."` message).
pub fn warning(vm: &mut Vm, dbh: &Dbh, message: &str) -> PResult<()> {
    if dbh.in_ctor.get() {
        let msg = format!("{}: {message}", vm.error_origin());
        return Err(throw(vm, msg));
    }
    vm.warning(message)
}

fn state_message(code: &ErrCode) -> &'static str {
    state_to_description(&err_str(code)).unwrap_or("<<Unknown error>>")
}

/// `pdo_raise_impl_error`: sets the SQLSTATE on the statement (or handle)
/// and reports it according to the error mode.
pub fn raise_impl_error(
    vm: &mut Vm,
    dbh: &Dbh,
    stmt: Option<&Stmt>,
    state: &str,
    supp: Option<&str>,
) -> PResult<()> {
    let code = sqlstate(state);
    match stmt {
        Some(s) => s.error_code.set(code),
        None => dbh.error_code.set(code),
    }
    let mode = dbh.error_mode.get();
    if mode == ERRMODE_SILENT {
        return Ok(());
    }
    let msg = state_message(&code);
    let message = match supp {
        Some(supp) => format!("SQLSTATE[{state}]: {msg}: {supp}"),
        None => format!("SQLSTATE[{state}]: {msg}"),
    };
    if mode != ERRMODE_EXCEPTION {
        return warning(vm, dbh, &message);
    }
    let ex = new_exception(vm, message, Value::from(state));
    let mut info = PhpArray::new();
    let _ = info.append(Value::from(state));
    let _ = info.append(Value::Int(0));
    ex.set_prop(b"errorInfo", Value::array(info));
    Err(Flow::Exception(ex))
}

/// `pdo_handle_error`: reports the current error of the statement (or
/// handle), with the driver's native code and message.
pub fn handle_error(vm: &mut Vm, dbh: &Dbh, stmt: Option<&Stmt>) -> PResult<()> {
    let mode = dbh.error_mode.get();
    if mode == ERRMODE_SILENT {
        return Ok(());
    }
    let code = match stmt {
        Some(s) => s.error_code.get(),
        None => dbh.error_code.get(),
    };
    let st = err_str(&code);
    let msg = state_message(&code);
    let mut native_code = 0i64;
    let mut supp: Option<PhpStr> = None;
    let mut info = None;
    if let Some(methods) = dbh.methods().filter(|m| m.supports_fetch_err()) {
        let mut arr = PhpArray::new();
        let _ = arr.append(Value::String(st.clone()));
        methods.fetch_err(dbh, stmt, &mut arr);
        if let Some(Value::Int(n)) = arr.get_int(1) {
            native_code = *n;
        }
        if let Some(v) = arr.get_int(2) {
            supp = Some(match v {
                Value::String(s) => s.clone(),
                other => {
                    PhpStr::from(php_runtime::value::convert::to_int_silent(other).to_string())
                }
            });
        }
        info = Some(arr);
    }
    let message = match (&supp, native_code) {
        (Some(supp), n) if n != 0 => format!("SQLSTATE[{st}]: {msg}: {n} {supp}"),
        (Some(supp), _) => format!("SQLSTATE[{st}]: {msg}: {supp}"),
        (None, _) => format!("SQLSTATE[{st}]: {msg}"),
    };
    if mode == ERRMODE_WARNING {
        return warning(vm, dbh, &message);
    }
    // `EG(exception) == NULL`: an exception thrown by a user callback while
    // the driver ran takes precedence.
    if dbh.methods().is_some_and(|m| m.has_exception()) {
        return Ok(());
    }
    let ex = new_exception(vm, message, Value::String(st));
    if let Some(info) = info {
        ex.set_prop(b"errorInfo", Value::array(info));
    }
    Err(Flow::Exception(ex))
}

/// `PDO_HANDLE_DBH_ERR()`.
pub fn handle_dbh_err(vm: &mut Vm, dbh: &Dbh) -> PResult<()> {
    if dbh.error_code.get() != ERR_NONE {
        handle_error(vm, dbh, None)?;
    }
    Ok(())
}

/// `PDO_HANDLE_STMT_ERR()`.
pub fn handle_stmt_err(vm: &mut Vm, dbh: &Dbh, stmt: &Stmt) -> PResult<()> {
    if stmt.error_code.get() != ERR_NONE {
        handle_error(vm, dbh, Some(stmt))?;
    }
    Ok(())
}

/// `zend_exception_set_previous(new, previous)`: appends `previous` at the
/// end of `new`'s chain, as `zend_throw_exception_internal()` does when an
/// exception is thrown while another one is pending.
pub fn chain(new: Flow, previous: Flow) -> Flow {
    let (Flow::Exception(n), Flow::Exception(p)) = (&new, &previous) else {
        return new;
    };
    if ObjRef::ptr_eq(n, p) {
        return new;
    }
    let mut cur = n.clone();
    let mut guard = 0;
    loop {
        match cur.get_prop(b"previous") {
            Some(Value::Object(o)) if guard < 10_000 => {
                if ObjRef::ptr_eq(&o, p) {
                    return new;
                }
                cur = o;
                guard += 1;
            }
            _ => break,
        }
    }
    cur.set_prop(b"previous", Value::Object(p.clone()));
    new
}

/// Registers `PDOException extends RuntimeException` (SPL); falls back to
/// `Exception` while SPL does not provide `RuntimeException`.
pub fn register(r: &mut Registry) {
    let parent = r
        .find_class("RuntimeException")
        .or_else(|| r.find_class("Exception"))
        .expect("Exception is registered by Core");
    let mut ce = ClassEntry::new_subclass(EXCEPTION_CLASS, 0, &parent);
    let mut code = PropertyInfo::new("code", EXCEPTION_CLASS, member_flags::PROTECTED);
    code.default = Value::Int(0);
    ce.declare_property(code);
    let mut info = PropertyInfo::new("errorInfo", EXCEPTION_CLASS, member_flags::PUBLIC);
    info.ty = Some(TypeDecl::parse("?array").expect("valid type"));
    info.default = Value::Null;
    ce.declare_property(info);
    r.class(ce);
}
