//! `basic_functions.c` system functions: `getenv`, `putenv`, `sleep`,
//! `usleep`, `time_nanosleep`, `time_sleep_until`, `get_current_user`,
//! `get_cfg_var`, `error_log`, `sys_getloadavg`, connection handling,
//! uploaded files and `set_time_limit` (`main/main.c`).

use std::io::Write;
use std::time::Duration;

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::{BasicState, path_arg, sys};
use crate::standard::{mail, pageinfo, syslog};

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

/// `valid_environment_name()`: names with ` `, `.` or `[` are not importable.
fn valid_environment_name(name: &[u8]) -> bool {
    !name.iter().any(|&c| c == b' ' || c == b'.' || c == b'[')
}

fn getenv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    if args.is_null(0) {
        // php_load_environment_variables()
        let mut a = PhpArray::new();
        for (k, v) in sys::env_vars() {
            if k.is_empty() || !valid_environment_name(&k) {
                continue;
            }
            a.insert(ArrayKey::from_bytes(&k), Value::bytes(&v));
        }
        return Ok(Value::array(a));
    }
    let name = path_arg(vm, args, 0)?;
    // `local_only` only matters for SAPIs with their own getenv hook (not the CLI).
    Ok(match sys::env_var(&name) {
        Some(v) => Value::bytes(&v),
        None => Value::Bool(false),
    })
}

fn putenv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let setting = path_arg(vm, args, 0)?;
    if setting.is_empty() || setting[0] == b'=' {
        return Err(vm.argument_value_error(1, "must have a valid syntax"));
    }
    // TODO(runtime): restore the original environment at request shutdown
    // (`BG(putenv_ht)`); the runtime has no module shutdown hook yet.
    match setting.iter().position(|&c| c == b'=') {
        Some(p) => sys::set_env(&setting[..p], &setting[p + 1..]),
        // No '=': the variable is unset.
        None => sys::unset_env(&setting),
    }
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------------------------
// Sleeping
// ---------------------------------------------------------------------------

fn sleep(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let num = args.int(0);
    if num < 0 || num as u64 > u32::MAX as u64 {
        return Err(vm.argument_value_error(1, "must be between 0 and 4294967295"));
    }
    std::thread::sleep(Duration::from_secs(num as u64));
    Ok(Value::Int(0))
}

fn usleep(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let num = args.int(0);
    if num < 0 || num as u64 > u32::MAX as u64 {
        return Err(vm.argument_value_error(1, "must be between 0 and 4294967295"));
    }
    std::thread::sleep(Duration::from_micros(num as u64));
    Ok(Value::Null)
}

fn time_nanosleep(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (sec, nsec) = (args.int(0), args.int(1));
    if sec < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    if !(0..=999_999_999).contains(&nsec) {
        return Err(vm.argument_value_error(2, "must be between 0 and 999999999"));
    }
    std::thread::sleep(Duration::new(sec as u64, nsec as u32));
    Ok(Value::Bool(true))
}

fn time_sleep_until(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    const NS_PER_SEC: u64 = 1_000_000_000;
    let target_secs = args.float(0);
    let top_target_sec = (u64::MAX / NS_PER_SEC) as f64;
    let (sec, usec) = crate::standard::microtime::gettimeofday_now();

    if !(target_secs >= 0.0 && target_secs <= top_target_sec) {
        return Err(vm.argument_value_error(
            1,
            &format!("must be between 0 and {}", top_target_sec as u64),
        ));
    }
    let target_ns = (target_secs * NS_PER_SEC as f64) as u64;
    let current_ns = sec as u64 * NS_PER_SEC + usec as u64 * 1000;
    if target_ns < current_ns {
        vm.warning("Argument #1 ($timestamp) must be greater than or equal to the current time")?;
        return Ok(Value::Bool(false));
    }
    let diff = target_ns - current_ns;
    std::thread::sleep(Duration::new(diff / NS_PER_SEC, (diff % NS_PER_SEC) as u32));
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------------------------
// Configuration and logging
// ---------------------------------------------------------------------------

fn get_current_user(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(pageinfo::current_user(vm))))
}

/// `add_config_entries()`: strings and (nested) arrays only.
fn config_entries(a: &PhpArray) -> PhpArray {
    let mut out = PhpArray::new();
    for (k, v) in a.iter() {
        match &*v.deref() {
            Value::String(s) => {
                out.insert(k, Value::String(s.clone()));
            }
            Value::Array(inner) => {
                out.insert(k, Value::array(config_entries(inner)));
            }
            _ => {}
        }
    }
    out
}

fn get_cfg_var(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let found = vm
        .ext_data::<BasicState>()
        .cfg
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.clone());
    Ok(match found {
        Some(Value::Array(a)) => Value::array(config_entries(&a)),
        Some(Value::String(s)) => Value::String(s),
        _ => Value::Bool(false),
    })
}

/// `php_log_err_with_severity()`: the `error_log` file, `syslog`, or the
/// SAPI's logger (stderr for the CLI).
pub(crate) fn php_log_err_with_severity(vm: &mut Vm, message: &[u8], severity: i64) {
    if vm.ext_data::<BasicState>().in_error_log {
        // prevent recursive invocation
        return;
    }
    vm.ext_data::<BasicState>().in_error_log = true;
    log_err(vm, message, severity);
    vm.ext_data::<BasicState>().in_error_log = false;
}

fn log_err(vm: &mut Vm, message: &[u8], severity: i64) {
    if let Some(error_log) = vm.ini_get("error_log").filter(|e| !e.is_empty()) {
        if error_log == "syslog" {
            syslog::php_syslog(vm, severity, message);
            return;
        }
        let mut mode = 0o644;
        let m = vm.ini_int("error_log_mode");
        if m > 0 && m <= 0o777 {
            mode = m as u32;
        }
        if let Some(mut f) = open_append(&error_log, mode) {
            let tz = super::default_timezone(vm);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let stamp = match php_timelib::builtin_tzinfo(&tz) {
                Some(info) => php_timelib::format_timestamp(b"d-M-Y H:i:s e", now, Some(&info)),
                None => b"".to_vec(),
            };
            let mut line = b"[".to_vec();
            line.extend_from_slice(&stamp);
            line.extend_from_slice(b"] ");
            line.extend_from_slice(message);
            line.push(b'\n');
            let _ = f.write_all(&line);
            return;
        }
    }
    // Otherwise fall back to the SAPI's logger.
    sapi_log_message(vm, message);
}

/// `sapi_cli_log_message()`.
fn sapi_log_message(vm: &mut Vm, message: &[u8]) {
    let mut m = message.to_vec();
    m.push(b'\n');
    vm.write_log(&m);
}

fn open_append(path: &PhpStr, mode: u32) -> Option<std::fs::File> {
    use std::fs::OpenOptions;
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(mode);
        o.open(std::ffi::OsStr::from_bytes(path.as_bytes())).ok()
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        o.open(String::from_utf8_lossy(path).as_ref()).ok()
    }
}

/// `_php_error_log()`; `Ok(false)` is php-src's FAILURE.
pub(crate) fn php_error_log(
    vm: &mut Vm,
    opt_err: i64,
    message: &PhpStr,
    opt: Option<&PhpStr>,
    headers: Option<&PhpStr>,
) -> PResult<bool> {
    match opt_err {
        1 => {
            // send an email
            let Some(to) = opt else { return Ok(false) };
            mail::php_mail(
                vm,
                to,
                b"PHP error_log message",
                message,
                headers.map(|h| &h[..]),
                None,
            )
        }
        2 => {
            // send to an address
            Err(vm.throw_value_error("TCP/IP option is not available for error logging"))
        }
        3 => {
            // save to a file
            let path = match opt {
                Some(p) if !p.is_empty() => p.clone(),
                _ => return Err(vm.throw_value_error("Path cannot be empty")),
            };
            append_to_stream(vm, &path, message)
        }
        4 => {
            // send to the SAPI logger directly
            sapi_log_message(vm, message);
            Ok(true)
        }
        _ => {
            php_log_err_with_severity(vm, message, 5 /* LOG_NOTICE */);
            Ok(true)
        }
    }
}

/// `php_stream_open_wrapper(path, "a", REPORT_ERRORS)` + write + close.
fn append_to_stream(vm: &mut Vm, path: &PhpStr, data: &[u8]) -> PResult<bool> {
    let p = path.as_bytes();
    // Stream wrappers that need no file.
    // TODO(streams): route through the streams module once it exposes an
    // open-for-write interface (user wrappers, php://temp, data:, ...).
    match p {
        b"php://stderr" => {
            vm.write_log(data);
            return Ok(true);
        }
        b"php://stdout" | b"php://output" => {
            vm.echo(data)?;
            return Ok(true);
        }
        _ => {}
    }
    let local = p.strip_prefix(b"file://").unwrap_or(p);
    match open_append(&PhpStr::from(local), 0o666) {
        Some(mut f) => Ok(f.write_all(data).is_ok()),
        None => {
            let err = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(String::from_utf8_lossy(local).as_ref())
                .err();
            let why = err
                .as_ref()
                .map_or_else(|| "Unknown error".to_string(), super::os_error_text);
            super::warning_docref1(vm, path, &format!("Failed to open stream: {why}"))?;
            Ok(false)
        }
    }
}

fn error_log(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let message = args.str(0).clone();
    let erropt = args.int(1);
    let opt = match args.str_opt(2) {
        Some(s) if s.contains(&0) => {
            return Err(vm.argument_value_error(3, "must not contain any null bytes"));
        }
        other => other.cloned(),
    };
    let headers = args.str_opt(3).cloned();
    // `(int) erropt`
    let ok = php_error_log(
        vm,
        erropt as i32 as i64,
        &message,
        opt.as_ref(),
        headers.as_ref(),
    )?;
    Ok(Value::Bool(ok))
}

fn sys_getloadavg(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(match sys::getloadavg() {
        Some(l) => {
            let mut a = PhpArray::new();
            for v in l {
                let _ = a.append(Value::Float(v));
            }
            Value::array(a)
        }
        None => Value::Bool(false),
    })
}

// ---------------------------------------------------------------------------
// Connection handling, uploads, time limit
// ---------------------------------------------------------------------------

/// `PHP_CONNECTION_*`.
pub const PHP_CONNECTION_NORMAL: i64 = 0;
pub const PHP_CONNECTION_ABORTED: i64 = 1;
pub const PHP_CONNECTION_TIMEOUT: i64 = 2;

fn connection_aborted(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(
        vm.ext_data::<BasicState>().connection_status & PHP_CONNECTION_ABORTED != 0,
    ))
}

fn connection_status(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(vm.ext_data::<BasicState>().connection_status))
}

fn ignore_user_abort(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let old = vm.ini_bool("ignore_user_abort");
    if let Some(arg) = args.bool_opt(0) {
        vm.ini_set("ignore_user_abort", if arg { "1" } else { "0" });
    }
    Ok(Value::Bool(old))
}

fn is_uploaded_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // CLI requests carry no rfc1867 uploads.
    path_arg(vm, args, 0)?;
    Ok(Value::Bool(false))
}

fn move_uploaded_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    path_arg(vm, args, 0)?;
    path_arg(vm, args, 1)?;
    Ok(Value::Bool(false))
}

fn set_time_limit(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let secs = args.int(0);
    Ok(Value::Bool(
        vm.ini_set("max_execution_time", secs.to_string()).is_some(),
    ))
}

pub static GETENV: NativeFunction = NativeFunction {
    sig: "getenv(?string $name = null, bool $local_only = false): string|array|false",
    handler: getenv,
};
pub static PUTENV: NativeFunction = NativeFunction {
    sig: "putenv(string $assignment): bool",
    handler: putenv,
};
pub static SLEEP: NativeFunction = NativeFunction {
    sig: "sleep(int $seconds): int",
    handler: sleep,
};
pub static USLEEP: NativeFunction = NativeFunction {
    sig: "usleep(int $microseconds): void",
    handler: usleep,
};
pub static TIME_NANOSLEEP: NativeFunction = NativeFunction {
    sig: "time_nanosleep(int $seconds, int $nanoseconds): array|bool",
    handler: time_nanosleep,
};
pub static TIME_SLEEP_UNTIL: NativeFunction = NativeFunction {
    sig: "time_sleep_until(float $timestamp): bool",
    handler: time_sleep_until,
};
pub static GET_CURRENT_USER: NativeFunction = NativeFunction {
    sig: "get_current_user(): string",
    handler: get_current_user,
};
pub static GET_CFG_VAR: NativeFunction = NativeFunction {
    sig: "get_cfg_var(string $option): string|array|false",
    handler: get_cfg_var,
};
pub static ERROR_LOG: NativeFunction = NativeFunction {
    sig: "error_log(string $message, int $message_type = 0, ?string $destination = null, ?string $additional_headers = null): bool",
    handler: error_log,
};
pub static SYS_GETLOADAVG: NativeFunction = NativeFunction {
    sig: "sys_getloadavg(): array|false",
    handler: sys_getloadavg,
};
pub static CONNECTION_ABORTED: NativeFunction = NativeFunction {
    sig: "connection_aborted(): bool",
    handler: connection_aborted,
};
pub static CONNECTION_STATUS: NativeFunction = NativeFunction {
    sig: "connection_status(): int",
    handler: connection_status,
};
pub static IGNORE_USER_ABORT: NativeFunction = NativeFunction {
    sig: "ignore_user_abort(?bool $enable = null): bool",
    handler: ignore_user_abort,
};
pub static IS_UPLOADED_FILE: NativeFunction = NativeFunction {
    sig: "is_uploaded_file(string $filename): bool",
    handler: is_uploaded_file,
};
pub static MOVE_UPLOADED_FILE: NativeFunction = NativeFunction {
    sig: "move_uploaded_file(string $from, string $to): bool",
    handler: move_uploaded_file,
};
pub static SET_TIME_LIMIT: NativeFunction = NativeFunction {
    sig: "set_time_limit(int $seconds): bool",
    handler: set_time_limit,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &GETENV,
        &PUTENV,
        &SLEEP,
        &USLEEP,
        &TIME_NANOSLEEP,
        &TIME_SLEEP_UNTIL,
        &GET_CURRENT_USER,
        &GET_CFG_VAR,
        &ERROR_LOG,
        &SYS_GETLOADAVG,
        &CONNECTION_ABORTED,
        &CONNECTION_STATUS,
        &IGNORE_USER_ABORT,
        &IS_UPLOADED_FILE,
        &MOVE_UPLOADED_FILE,
        &SET_TIME_LIMIT,
    ]);
    r.constant("CONNECTION_ABORTED", PHP_CONNECTION_ABORTED);
    r.constant("CONNECTION_NORMAL", PHP_CONNECTION_NORMAL);
    r.constant("CONNECTION_TIMEOUT", PHP_CONNECTION_TIMEOUT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::standard::basic::set_cfg_entry;
    use crate::testing::{TestVm, thrown};

    fn s(v: &Value) -> String {
        v.as_str()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap()
    }

    #[test]
    fn getenv_putenv_roundtrip() {
        let mut t = TestVm::new();
        assert!(matches!(
            t.call("getenv", vec![Value::str("PHP_IN_RUST_TEST_VAR_1")])
                .unwrap(),
            Value::Bool(false)
        ));
        let r = t
            .call("putenv", vec![Value::str("PHP_IN_RUST_TEST_VAR_1=bar")])
            .unwrap();
        assert!(matches!(r, Value::Bool(true)));
        assert_eq!(
            s(
                &t.call("getenv", vec![Value::str("PHP_IN_RUST_TEST_VAR_1")])
                    .unwrap()
            ),
            "bar"
        );
        let all = t.call("getenv", vec![]).unwrap();
        let Value::Array(a) = all else {
            panic!("array")
        };
        assert_eq!(
            a.get_str(b"PHP_IN_RUST_TEST_VAR_1")
                .unwrap()
                .as_str()
                .unwrap(),
            "bar"
        );
        // No '=': unset
        t.call("putenv", vec![Value::str("PHP_IN_RUST_TEST_VAR_1")])
            .unwrap();
        assert!(matches!(
            t.call("getenv", vec![Value::str("PHP_IN_RUST_TEST_VAR_1")])
                .unwrap(),
            Value::Bool(false)
        ));
    }

    #[test]
    fn putenv_validation() {
        let mut t = TestVm::new();
        for bad in ["", "=x"] {
            let (c, m) = thrown(t.call("putenv", vec![Value::str(bad)]));
            assert_eq!(c, "ValueError");
            assert_eq!(
                m,
                "putenv(): Argument #1 ($assignment) must have a valid syntax"
            );
        }
        let (c, m) = thrown(t.call("putenv", vec![Value::bytes(b"A\0=1")]));
        assert_eq!(c, "ValueError");
        assert_eq!(
            m,
            "putenv(): Argument #1 ($assignment) must not contain any null bytes"
        );
    }

    #[test]
    fn sleep_argument_errors() {
        let mut t = TestVm::new();
        let (c, m) = thrown(t.call("sleep", vec![Value::Int(-1)]));
        assert_eq!(c, "ValueError");
        assert_eq!(
            m,
            "sleep(): Argument #1 ($seconds) must be between 0 and 4294967295"
        );
        let (_, m) = thrown(t.call("usleep", vec![Value::Int(-1)]));
        assert_eq!(
            m,
            "usleep(): Argument #1 ($microseconds) must be between 0 and 4294967295"
        );
        let (_, m) = thrown(t.call("time_nanosleep", vec![Value::Int(-2), Value::Int(1000)]));
        assert_eq!(
            m,
            "time_nanosleep(): Argument #1 ($seconds) must be greater than or equal to 0"
        );
        let (_, m) = thrown(t.call(
            "time_nanosleep",
            vec![Value::Int(1), Value::Int(1_000_000_000)],
        ));
        assert_eq!(
            m,
            "time_nanosleep(): Argument #2 ($nanoseconds) must be between 0 and 999999999"
        );
        let (_, m) = thrown(t.call("time_sleep_until", vec![Value::Float(-1.0)]));
        assert!(
            m.starts_with("time_sleep_until(): Argument #1 ($timestamp) must be between 0 and "),
            "{m}"
        );
    }

    #[test]
    fn sleeping_works() {
        let mut t = TestVm::new();
        assert_eq!(
            t.call("sleep", vec![Value::Int(0)]).unwrap().as_int(),
            Some(0)
        );
        assert!(matches!(
            t.call("usleep", vec![Value::Int(10)]).unwrap(),
            Value::Null
        ));
        assert!(matches!(
            t.call("time_nanosleep", vec![Value::Int(0), Value::Int(1000)])
                .unwrap(),
            Value::Bool(true)
        ));
    }

    #[test]
    fn time_sleep_until_in_the_past_warns() {
        let mut t = TestVm::new();
        let r = t.call("time_sleep_until", vec![Value::Float(1.0)]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: time_sleep_until(): Argument #1 ($timestamp) must be greater than or equal to the current time in /t.php on line 3\n"
        );
    }

    #[test]
    fn time_sleep_until_future() {
        let mut t = TestVm::new();
        let (sec, usec) = crate::standard::microtime::gettimeofday_now();
        let target = sec as f64 + usec as f64 / 1e6 + 0.01;
        assert!(matches!(
            t.call("time_sleep_until", vec![Value::Float(target)])
                .unwrap(),
            Value::Bool(true)
        ));
    }

    #[test]
    fn connection_functions() {
        let mut t = TestVm::new();
        assert_eq!(
            t.call("connection_status", vec![]).unwrap().as_int(),
            Some(0)
        );
        assert!(matches!(
            t.call("connection_aborted", vec![]).unwrap(),
            Value::Bool(false)
        ));
        assert_eq!(
            t.vm.constant("CONNECTION_TIMEOUT").unwrap().as_int(),
            Some(2)
        );
    }

    #[test]
    fn ignore_user_abort_returns_old_value() {
        let mut t = TestVm::new();
        assert!(matches!(
            t.call("ignore_user_abort", vec![]).unwrap(),
            Value::Bool(false)
        ));
        assert!(matches!(
            t.call("ignore_user_abort", vec![Value::Bool(true)])
                .unwrap(),
            Value::Bool(false)
        ));
        assert!(matches!(
            t.call("ignore_user_abort", vec![Value::Null]).unwrap(),
            Value::Bool(true)
        ));
        assert_eq!(t.vm.ini_get("ignore_user_abort").unwrap(), "1");
    }

    #[test]
    fn set_time_limit_alters_ini() {
        let mut t = TestVm::new();
        assert!(matches!(
            t.call("set_time_limit", vec![Value::Int(15)]).unwrap(),
            Value::Bool(true)
        ));
        assert_eq!(t.vm.ini_get("max_execution_time").unwrap(), "15");
    }

    #[test]
    fn error_log_default_goes_to_stderr_sink() {
        let mut t = TestVm::new();
        let r = t.call("error_log", vec![Value::str("hello")]).unwrap();
        assert!(matches!(r, Value::Bool(true)));
        assert_eq!(t.log.borrow().as_slice(), b"hello\n");
        t.log.borrow_mut().clear();
        t.call("error_log", vec![Value::str("sapi"), Value::Int(4)])
            .unwrap();
        assert_eq!(t.log.borrow().as_slice(), b"sapi\n");
    }

    #[test]
    fn error_log_to_file_and_options() {
        let mut t = TestVm::new();
        let path =
            std::env::temp_dir().join(format!("php_in_rust_error_log_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let p = Value::str(path.to_str().unwrap());
        let r = t
            .call(
                "error_log",
                vec![Value::str("first\n"), Value::Int(3), p.clone()],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(true)));
        t.call(
            "error_log",
            vec![Value::str("second\n"), Value::Int(3), p.clone()],
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        let _ = std::fs::remove_file(&path);

        // the error_log ini setting
        t.vm.ini_set_system("error_log", path.to_str().unwrap());
        t.call("error_log", vec![Value::str("via ini")]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with('[') && text.ends_with("UTC] via ini\n"),
            "{text}"
        );
        let _ = std::fs::remove_file(&path);

        // option 2 is gone
        let (c, m) = thrown(t.call("error_log", vec![Value::str("x"), Value::Int(2)]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            (
                "ValueError",
                "TCP/IP option is not available for error logging"
            )
        );
        // option 3 needs a path
        let (c, m) = thrown(t.call("error_log", vec![Value::str("x"), Value::Int(3)]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            ("ValueError", "Path cannot be empty")
        );
        // option 1 without destination fails
        assert!(matches!(
            t.call("error_log", vec![Value::str("x"), Value::Int(1)])
                .unwrap(),
            Value::Bool(false)
        ));
    }

    #[test]
    fn error_log_unwritable_file_warns() {
        let mut t = TestVm::new();
        let r = t
            .call(
                "error_log",
                vec![
                    Value::str("x"),
                    Value::Int(3),
                    Value::str("/nonexistent-dir/log.txt"),
                ],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: error_log(/nonexistent-dir/log.txt): Failed to open stream: No such file or directory in /t.php on line 3\n"
        );
    }

    #[test]
    fn get_cfg_var_reads_the_configuration_hash() {
        let mut t = TestVm::new();
        assert!(matches!(
            t.call("get_cfg_var", vec![Value::str("foo")]).unwrap(),
            Value::Bool(false)
        ));
        set_cfg_entry(&mut t.vm, "foo", Value::str("bar"));
        assert_eq!(
            s(&t.call("get_cfg_var", vec![Value::str("foo")]).unwrap()),
            "bar"
        );
    }

    #[test]
    fn loadavg_and_uploads() {
        let mut t = TestVm::new();
        if let Value::Array(a) = t.call("sys_getloadavg", vec![]).unwrap() {
            assert_eq!(a.len(), 3);
            assert!(matches!(a.get_int(0), Some(Value::Float(_))));
        }
        assert!(matches!(
            t.call("is_uploaded_file", vec![Value::str("/tmp/x")])
                .unwrap(),
            Value::Bool(false)
        ));
        assert!(matches!(
            t.call(
                "move_uploaded_file",
                vec![Value::str("/tmp/x"), Value::str("/tmp/y")]
            )
            .unwrap(),
            Value::Bool(false)
        ));
    }
}
