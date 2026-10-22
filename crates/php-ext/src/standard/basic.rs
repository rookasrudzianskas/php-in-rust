//! ext/standard/basic_functions.c (and the parts of `main/main.c` it relies on).
//!
//! * [`netfuncs`] — `ip2long`, `inet_ntop`, `getservbyname`, ...
//! * [`misc`] — `getenv`, `sleep`, `error_log`, `get_cfg_var`, ...
//! * [`getopt`] — `getopt()` and `php_getopt()`.
//! * [`ini`] — `ini_get`, `ini_set`, `ini_get_all`, `ini_parse_quantity`, the
//!   include path functions.
//! * [`highlight`] — `highlight_file`, `highlight_string`, `php_strip_whitespace`.
//! * [`sys`] — the libc calls the standard library does not offer.

pub mod getopt;
pub mod highlight;
pub mod ini;
pub mod misc;
pub mod netfuncs;
pub mod sys;

use php_runtime::{Args, PResult, PhpStr, Registry, Value, Vm};

/// Per-request state of the `basic_functions` module (`basic_globals`) plus
/// the SAPI information it needs.
#[derive(Default)]
pub struct BasicState {
    /// The primary script (`SG(request_info).path_translated`). The CLI sets
    /// it through [`set_main_script`]; `getmyuid()`, `getlastmod()`,
    /// `get_current_user()` and friends stat it.
    pub main_script: Option<PhpStr>,
    /// `$_SERVER['argv']`, set by the CLI through [`set_argv`]; `getopt()`
    /// reads it (it returns `false` while unset).
    pub argv: Option<Vec<PhpStr>>,
    /// The parsed php.ini / `-d` configuration (`cfg_get_entry()`), as
    /// `get_cfg_var()` reports it. Filled with [`set_cfg_entry`].
    pub cfg: Vec<(PhpStr, Value)>,
    /// `$_SERVER['HTTP_USER_AGENT']` for `get_browser()`; the interpreter
    /// records it with [`set_http_user_agent`] when the superglobal exists.
    pub http_user_agent: Option<PhpStr>,
    /// `php_ini_opened_path`, `php_ini_scanned_path`, `php_ini_scanned_files`
    /// (set by the CLI through [`set_ini_files`]).
    pub ini_opened_path: Option<PhpStr>,
    pub ini_scanned_path: Option<PhpStr>,
    pub ini_scanned_files: Option<PhpStr>,
    /// `PG(connection_status)`.
    pub connection_status: i64,
    /// `PG(in_error_log)`: guards `php_log_err()` against recursion.
    pub in_error_log: bool,
}

/// Records the primary script's path (`SG(request_info).path_translated`).
pub fn set_main_script(vm: &mut Vm, path: impl Into<PhpStr>) {
    vm.ext_data::<BasicState>().main_script = Some(path.into());
}

/// Records `$_SERVER['argv']` for `getopt()`.
pub fn set_argv(vm: &mut Vm, argv: Vec<PhpStr>) {
    vm.ext_data::<BasicState>().argv = Some(argv);
}

/// Records the php.ini files the SAPI loaded (`php_ini_loaded_file()`,
/// `php_ini_scanned_files()` and the `phpinfo()` rows).
pub fn set_ini_files(
    vm: &mut Vm,
    opened_path: Option<PhpStr>,
    scanned_path: Option<PhpStr>,
    scanned_files: Option<PhpStr>,
) {
    let st = vm.ext_data::<BasicState>();
    st.ini_opened_path = opened_path;
    st.ini_scanned_path = scanned_path;
    st.ini_scanned_files = scanned_files;
}

/// Records `$_SERVER['HTTP_USER_AGENT']` for `get_browser()`.
pub fn set_http_user_agent(vm: &mut Vm, ua: impl Into<PhpStr>) {
    vm.ext_data::<BasicState>().http_user_agent = Some(ua.into());
}

/// Records a php.ini / `-d` entry for `get_cfg_var()`. `value` is a string,
/// or an array of strings (`name[key]=value` entries).
pub fn set_cfg_entry(vm: &mut Vm, name: impl Into<PhpStr>, value: Value) {
    let name = name.into();
    let cfg = &mut vm.ext_data::<BasicState>().cfg;
    match cfg.iter_mut().find(|(k, _)| *k == name) {
        Some(e) => e.1 = value,
        None => cfg.push((name, value)),
    }
}

/// Path of the script that `sapi_get_stat()` would stat: the primary script
/// when the CLI recorded it, otherwise the file being executed.
pub(crate) fn script_path(vm: &mut Vm) -> Option<PhpStr> {
    if let Some(p) = vm.ext_data::<BasicState>().main_script.clone() {
        return Some(p);
    }
    vm.executing.then(|| vm.current_file.clone())
}

/// The default timezone identifier (`guess_timezone`): `date.timezone` when
/// it names a valid zone, else `UTC`.
// TODO(date): honour `date_default_timezone_set()` once the date module
// exposes its state.
pub(crate) fn default_timezone(vm: &Vm) -> String {
    match vm.ini_get("date.timezone") {
        Some(tz) if !tz.is_empty() && php_timelib::timezone_id_is_valid(&tz.to_string_lossy()) => {
            tz.to_string_lossy().into_owned()
        }
        _ => "UTC".to_string(),
    }
}

/// `Z_PARAM_PATH`: a string argument without NUL bytes, else the
/// `ValueError` php-src throws.
pub(crate) fn path_arg(vm: &mut Vm, args: &Args, i: usize) -> PResult<PhpStr> {
    let s = args.str(i).clone();
    if s.contains(&0) {
        return Err(vm.argument_value_error(i + 1, "must not contain any null bytes"));
    }
    Ok(s)
}

/// `php_error_docref1(NULL, param, E_WARNING, ...)`: the message is prefixed
/// `"fname(param): "`.
pub(crate) fn warning_docref1(vm: &mut Vm, param: &[u8], msg: &str) -> PResult<()> {
    let fname = vm.current_function_display_name();
    vm.zend_error(
        php_runtime::errors::level::E_WARNING,
        format!("{fname}({}): {msg}", String::from_utf8_lossy(param)),
    )
}

/// `strerror(errno)` for an I/O error (without Rust's ` (os error N)` suffix).
pub(crate) fn os_error_text(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.rfind(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// `highlight_*` / `ini_*` need to open script files the way
/// `zend_stream_open` does: plain paths go through `std::fs` (searching
/// `include_path` for relative names), URLs go through the streams module.
///
/// Returns `Ok(None)` after emitting `"Failed to open stream"`.
pub(crate) fn open_script_source(
    vm: &mut Vm,
    path: &PhpStr,
    use_include_path: bool,
) -> PResult<Option<Vec<u8>>> {
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;

    let bytes = path.as_bytes();
    let has_scheme = bytes
        .iter()
        .position(|&c| !(c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.')))
        .is_some_and(|i| i > 1 && bytes[i..].starts_with(b"://"))
        || bytes.starts_with(b"data:");
    if has_scheme && !bytes.starts_with(b"file://") {
        // TODO(streams): relies on the streams package (`todo!()` until then).
        return crate::standard::streams::read_all(vm, bytes);
    }
    let local = bytes.strip_prefix(b"file://").unwrap_or(bytes);
    if local.is_empty() {
        vm.warning("Failed to open stream: No such file or directory")?;
        return Ok(None);
    }

    let mut candidates: Vec<Vec<u8>> = vec![local.to_vec()];
    let explicitly_relative = local.starts_with(b"./") || local.starts_with(b"../");
    if use_include_path && local[0] != b'/' && !explicitly_relative {
        candidates.clear();
        let ip = vm.ini_get("include_path").unwrap_or_default();
        for dir in ip.split(|&c| c == b':') {
            if dir.is_empty() {
                continue;
            }
            let mut p = dir.to_vec();
            p.push(b'/');
            p.extend_from_slice(local);
            candidates.push(p);
        }
        candidates.push(local.to_vec());
    }
    let mut last_err = None;
    for c in &candidates {
        #[cfg(unix)]
        let p = OsStr::from_bytes(c);
        #[cfg(not(unix))]
        let owned = String::from_utf8_lossy(c).into_owned();
        #[cfg(not(unix))]
        let p = OsStr::new(&owned);
        match std::fs::read(p) {
            Ok(data) => return Ok(Some(data)),
            Err(e) => last_err = Some(e),
        }
    }
    let why = last_err.map_or_else(
        || "No such file or directory".to_string(),
        |e| os_error_text(&e),
    );
    vm.warning(format!("Failed to open stream: {why}"))?;
    Ok(None)
}

pub fn register(r: &mut Registry) {
    netfuncs::register(r);
    misc::register(r);
    getopt::register(r);
    ini::register(r);
    highlight::register(r);
}
