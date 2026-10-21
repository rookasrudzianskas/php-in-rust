//! ext/standard/basic_functions.c

pub mod sys;

use php_runtime::{PhpStr, Registry, Vm};

/// Per-request state of the `basic_functions` module (`basic_globals`).
#[derive(Default)]
pub struct BasicState {
    /// The primary script (`SG(request_info).path_translated`). The CLI sets
    /// it through [`set_main_script`]; `getmyuid()`, `getlastmod()`,
    /// `get_current_user()` and friends stat it.
    pub main_script: Option<PhpStr>,
}

/// Records the primary script's path (`SG(request_info).path_translated`).
pub fn set_main_script(vm: &mut Vm, path: impl Into<PhpStr>) {
    vm.ext_data::<BasicState>().main_script = Some(path.into());
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

pub fn register(_r: &mut Registry) {}
