//! ext/standard/assert.c

use php_runtime::errors::level;
use php_runtime::ini::{IniDef, modifiable, parse_bool};
use php_runtime::{
    Args, ClassEntry, Flow, NativeFunction, ObjRef, PResult, PhpStr, Registry, Value, Vm,
};

pub const ASSERT_ACTIVE: i64 = 1;
pub const ASSERT_CALLBACK: i64 = 2;
pub const ASSERT_BAIL: i64 = 3;
pub const ASSERT_WARNING: i64 = 4;
pub const ASSERT_EXCEPTION: i64 = 5;

/// `ASSERTG(callback)`: set by `assert_options(ASSERT_CALLBACK, ...)` and by
/// a runtime `ini_set('assert.callback', ...)`.
#[derive(Default)]
struct AssertState {
    callback: Option<Value>,
}

const ASSERT_INI: &[IniDef] = &[
    IniDef::new("assert.active", Some("1"), modifiable::ALL),
    IniDef::new("assert.bail", Some("0"), modifiable::ALL),
    IniDef::new("assert.warning", Some("1"), modifiable::ALL),
    IniDef::new("assert.callback", None, modifiable::ALL),
    IniDef::new("assert.exception", Some("1"), modifiable::ALL),
];

/// `ASSERTG(cb)`: the callback as configured before the script started.
fn startup_callback(vm: &Vm) -> Option<PhpStr> {
    let e = vm.ini.get(b"assert.callback")?;
    let v = match &e.orig_value {
        Some(orig) => orig.clone(),
        None => e.value.clone(),
    };
    v.filter(|s| !s.is_empty())
}

/// The `OnUpdate*Bool` / `OnChangeCallback` handlers run when an `assert.*`
/// directive is changed by `ini_set()` or `ini_restore()` (their runtime
/// stage): they keep `ASSERTG` in sync and emit the 8.3 deprecations.
pub(crate) fn on_ini_modified(vm: &mut Vm, name: &[u8], new_value: Option<&PhpStr>) -> PResult<()> {
    let flag = |v: Option<&PhpStr>| v.is_some_and(|v| parse_bool(v));
    match name {
        b"assert.callback" => {
            vm.ext_data::<AssertState>().callback = None;
            if let Some(v) = new_value.filter(|v| !v.is_empty()) {
                vm.deprecated("assert.callback INI setting is deprecated")?;
                vm.ext_data::<AssertState>().callback = Some(Value::String(v.clone()));
            }
        }
        b"assert.active" if !flag(new_value) => {
            vm.deprecated("assert.active INI setting is deprecated")?;
        }
        b"assert.bail" if flag(new_value) => {
            vm.deprecated("assert.bail INI setting is deprecated")?;
        }
        b"assert.exception" if !flag(new_value) => {
            vm.deprecated("assert.exception INI setting is deprecated")?;
        }
        b"assert.warning" if !flag(new_value) => {
            vm.deprecated("assert.warning INI setting is deprecated")?;
        }
        _ => {}
    }
    Ok(())
}

/// The effective callback (`ASSERTG(callback)`, falling back to `ASSERTG(cb)`).
fn current_callback(vm: &mut Vm) -> Option<Value> {
    if let Some(cb) = vm.ext_data::<AssertState>().callback.clone() {
        return Some(cb);
    }
    let cb = startup_callback(vm)?;
    let v = Value::String(cb);
    vm.ext_data::<AssertState>().callback = Some(v.clone());
    Some(v)
}

fn assert_fn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // Only reachable with zend.assertions <= 0 through dynamic calls.
    if !vm.ini_bool("assert.active") || vm.ini_int("zend.assertions") <= 0 {
        return Ok(Value::Bool(true));
    }

    let assertion = args.get(0).clone();
    if vm.to_bool(&assertion)? {
        return Ok(Value::Bool(true));
    }

    let (description_str, description_obj): (Option<PhpStr>, Option<ObjRef>) = match args.get(1) {
        Value::String(s) => (Some(s.clone()), None),
        Value::Object(o) => (None, Some(o.clone())),
        _ => (None, None),
    };

    if let Some(obj) = description_obj {
        return Err(Flow::Exception(obj));
    }

    let mut pending: Option<ObjRef> = None;
    if let Some(callback) = current_callback(vm) {
        let filename = if vm.executing {
            vm.current_file.clone()
        } else {
            PhpStr::from("Unknown")
        };
        let mut cb_args = vec![
            Value::String(filename),
            Value::Int(vm.current_line as i64),
            Value::Null,
        ];
        if let Some(d) = &description_str {
            cb_args.push(Value::String(d.clone()));
        }
        match vm.call(&callback, cb_args) {
            Ok(_) => {}
            // The exception stays pending, like EG(exception) in php-src.
            Err(Flow::Exception(e)) => pending = Some(e),
            Err(other) => return Err(other),
        }
    }

    let bail = vm.ini_bool("assert.bail");
    let mut thrown: Option<ObjRef> = None;
    if vm.ini_bool("assert.exception") {
        let msg = description_str.clone().unwrap_or_default();
        let ex = vm.create_exception("AssertionError", msg, level::E_ERROR);
        if let Some(prev) = pending.take() {
            // zend_exception_set_previous(new, old)
            ex.set_prop(b"previous", Value::Object(prev));
        }
        if bail {
            // "When bail is turned on, the exception will not be caught."
            vm.report_uncaught_exception(&ex)?;
        }
        thrown = Some(ex);
    } else if vm.ini_bool("assert.warning") {
        let what = description_str.as_ref().map_or_else(
            || "Assertion".to_string(),
            |d| d.to_string_lossy().into_owned(),
        );
        vm.warning(format!("{what} failed"))?;
    }

    if bail {
        if let Some(e) = pending.take().or(thrown.take()) {
            // The callback (or the printing above) left an exception pending:
            // show it as a warning so execution can unwind without a bailout.
            report_as_warning(vm, &e)?;
        }
        return Err(Flow::Exit(vm.exit_status));
    }
    match thrown.or(pending) {
        Some(e) => Err(Flow::Exception(e)),
        None => Ok(Value::Bool(false)),
    }
}

/// `zend_exception_error(ex, E_WARNING)`.
fn report_as_warning(vm: &mut Vm, ex: &ObjRef) -> PResult<()> {
    let s = match vm.call_method(ex, b"__tostring", Vec::new()) {
        Ok(v) => match v.into_unref() {
            Value::String(s) => s,
            _ => PhpStr::new(),
        },
        Err(Flow::Exception(_)) | Err(Flow::Exit(_)) => return Ok(()),
        Err(e) => return Err(e),
    };
    let file = match ex.get_prop(b"file") {
        Some(Value::String(f)) if !f.is_empty() => f,
        _ => PhpStr::from("Unknown"),
    };
    let line = ex.get_prop(b"line").and_then(|v| v.as_int()).unwrap_or(0);
    let mut msg = b"Uncaught ".to_vec();
    msg.extend_from_slice(&s);
    msg.extend_from_slice(b"\n  thrown");
    vm.zend_error_at(level::E_WARNING, file, line as u32, PhpStr::from_vec(msg))
}

fn assert_options(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.engine_deprecated("Function assert_options() is deprecated since 8.3")?;
    let what = args.int(0);
    let has_value = args.is_passed(1);

    let flag_option = |vm: &mut Vm, args: &Args, ini: &str| -> PResult<Value> {
        let old = vm.ini_bool(ini);
        if has_value {
            let v = args.get(1).clone();
            let s = vm.to_php_string(&v)?;
            // ZEND_INI_STAGE_ASSERT_OPTIONS: no deprecation from the handler.
            vm.ini_set(ini, s);
        }
        Ok(Value::Int(old as i64))
    };

    match what {
        ASSERT_ACTIVE => flag_option(vm, args, "assert.active"),
        ASSERT_BAIL => flag_option(vm, args, "assert.bail"),
        ASSERT_WARNING => flag_option(vm, args, "assert.warning"),
        ASSERT_EXCEPTION => flag_option(vm, args, "assert.exception"),
        ASSERT_CALLBACK => {
            let ret = match vm.ext_data::<AssertState>().callback.clone() {
                Some(cb) => cb,
                None => match startup_callback(vm) {
                    Some(s) => Value::String(s),
                    None => Value::Null,
                },
            };
            if has_value {
                let v = args.get(1).clone();
                vm.ext_data::<AssertState>().callback = match v {
                    Value::Null => None,
                    v => Some(v),
                };
            }
            Ok(ret)
        }
        _ => Err(vm.argument_value_error(1, "must be an ASSERT_* constant")),
    }
}

pub static ASSERT: NativeFunction = NativeFunction {
    sig: "assert(mixed $assertion, Throwable|string|null $description = null): bool",
    handler: assert_fn,
};
// `#[\Deprecated(since: '8.3')]`: the deprecation is raised by the handler.
pub static ASSERT_OPTIONS: NativeFunction = NativeFunction {
    sig: "assert_options(int $option, mixed $value = UNKNOWN): mixed",
    handler: assert_options,
};

pub fn register(r: &mut Registry) {
    r.ini_entries(ASSERT_INI);

    let error = r.find_class("Error").expect("Error is registered by Core");
    r.class(ClassEntry::new_subclass("AssertionError", 0, &error));

    r.functions(&[&ASSERT, &ASSERT_OPTIONS]);

    const MSG: &str = "since 8.3, as assert_options() is deprecated";
    r.deprecated_constant("ASSERT_ACTIVE", ASSERT_ACTIVE, MSG);
    r.deprecated_constant("ASSERT_CALLBACK", ASSERT_CALLBACK, MSG);
    r.deprecated_constant("ASSERT_BAIL", ASSERT_BAIL, MSG);
    r.deprecated_constant("ASSERT_WARNING", ASSERT_WARNING, MSG);
    r.deprecated_constant("ASSERT_EXCEPTION", ASSERT_EXCEPTION, MSG);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn assert_call(t: &mut TestVm, v: Value, desc: &str) -> PResult<Value> {
        t.call("assert", vec![v, Value::str(desc)])
    }

    #[test]
    fn passing_assertion_returns_true() {
        let mut t = TestVm::new();
        assert!(matches!(
            assert_call(&mut t, Value::Bool(true), "assert(true)").unwrap(),
            Value::Bool(true)
        ));
    }

    #[test]
    fn failure_throws_assertion_error_with_description() {
        let mut t = TestVm::new();
        let r = assert_call(&mut t, Value::Bool(false), "assert(false)");
        let (class, msg) = thrown(r);
        assert_eq!(class, "AssertionError");
        assert_eq!(msg, "assert(false)");
    }

    #[test]
    fn warning_mode_matches_assert_return_value_phpt() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("assert.exception", "0");
        let r = assert_call(&mut t, Value::Bool(false), "assert(false)").unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: assert(): assert(false) failed in /t.php on line 3\n"
        );
    }

    #[test]
    fn warning_without_description() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("assert.exception", "0");
        t.call("assert", vec![Value::Int(0), Value::Null]).unwrap();
        assert_eq!(
            t.output(),
            "\nWarning: assert(): Assertion failed in /t.php on line 3\n"
        );
    }

    #[test]
    fn dynamic_call_ignores_disabled_assertions() {
        // GH-18509
        let mut t = TestVm::new();
        t.vm.ini_set_system("zend.assertions", "0");
        let r = t.call("assert", vec![Value::Bool(false)]).unwrap();
        assert!(matches!(r, Value::Bool(true)));
    }

    #[test]
    fn inactive_assertions_pass() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("assert.active", "0");
        let r = t.call("assert", vec![Value::Bool(false)]).unwrap();
        assert!(matches!(r, Value::Bool(true)));
    }

    #[test]
    fn throwable_description_is_thrown() {
        let mut t = TestVm::new();
        let ex = t.vm.create_exception("Exception", "custom", 0);
        let r = t.call("assert", vec![Value::Bool(false), Value::Object(ex)]);
        let (class, msg) = thrown(r);
        assert_eq!((class.as_str(), msg.as_str()), ("Exception", "custom"));
    }

    #[test]
    fn assert_options_deprecated_and_unknown_option() {
        let mut t = TestVm::new();
        let r = t.call("assert_options", vec![Value::Int(1000)]);
        let (class, msg) = thrown(r);
        assert_eq!(class, "ValueError");
        assert_eq!(
            msg,
            "assert_options(): Argument #1 ($option) must be an ASSERT_* constant"
        );
        assert_eq!(
            t.output(),
            "\nDeprecated: Function assert_options() is deprecated since 8.3 in /t.php on line 3\n"
        );
    }

    #[test]
    fn assert_options_flags_roundtrip() {
        let mut t = TestVm::new();
        assert_eq!(
            t.call("assert_options", vec![Value::Int(ASSERT_ACTIVE)])
                .unwrap()
                .as_int(),
            Some(1)
        );
        // Setting returns the previous value and updates the ini entry.
        assert_eq!(
            t.call(
                "assert_options",
                vec![Value::Int(ASSERT_ACTIVE), Value::Int(0)]
            )
            .unwrap()
            .as_int(),
            Some(1)
        );
        assert_eq!(t.vm.ini_get("assert.active").unwrap(), "0");
        assert_eq!(
            t.call("assert_options", vec![Value::Int(ASSERT_WARNING)])
                .unwrap()
                .as_int(),
            Some(1)
        );
        assert_eq!(
            t.call("assert_options", vec![Value::Int(ASSERT_BAIL)])
                .unwrap()
                .as_int(),
            Some(0)
        );
    }

    #[test]
    fn assert_options_callback_and_startup_value() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("assert.callback", "f1");
        // assert_basic4.phpt: the configured callback is visible
        let v = t
            .call("assert_options", vec![Value::Int(ASSERT_CALLBACK)])
            .unwrap();
        assert_eq!(v.as_str().unwrap(), "f1");
        let old = t
            .call(
                "assert_options",
                vec![Value::Int(ASSERT_CALLBACK), Value::str("g")],
            )
            .unwrap();
        assert_eq!(old.as_str().unwrap(), "f1");
        let now = t
            .call("assert_options", vec![Value::Int(ASSERT_CALLBACK)])
            .unwrap();
        assert_eq!(now.as_str().unwrap(), "g");
        // ini_get is unaffected by assert_options()
        assert_eq!(t.vm.ini_get("assert.callback").unwrap(), "f1");
    }

    #[test]
    fn ini_deprecations() {
        let mut t = TestVm::new();
        on_ini_modified(&mut t.vm, b"assert.active", Some(&PhpStr::from("0"))).unwrap();
        assert!(
            t.output()
                .contains("Deprecated: main(): assert.active INI setting is deprecated")
        );
        on_ini_modified(&mut t.vm, b"assert.bail", Some(&PhpStr::from("0"))).unwrap();
        assert_eq!(t.output(), "");
        on_ini_modified(&mut t.vm, b"assert.callback", Some(&PhpStr::from("cb"))).unwrap();
        assert!(
            t.output()
                .contains("assert.callback INI setting is deprecated")
        );
    }

    #[test]
    fn bail_prints_warning_then_exits() {
        // assert04.phpt: "not reached" is never printed
        let mut t = TestVm::new();
        t.vm.ini_set_system("assert.exception", "0");
        t.vm.ini_set_system("assert.bail", "1");
        let r = assert_call(&mut t, Value::Int(0), "assert(0)");
        assert!(matches!(r, Err(Flow::Exit(_))), "{r:?}");
        assert_eq!(
            t.output(),
            "\nWarning: assert(): assert(0) failed in /t.php on line 3\n"
        );
    }

    #[test]
    fn callback_receives_file_line_null_and_description() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("assert.exception", "0");
        t.vm.ini_set_system("assert.warning", "0");
        // A native callback is enough to observe that it is invoked:
        // `getmypid` with four arguments throws ArgumentCountError.
        t.vm.ini_set_system("assert.callback", "getmypid");
        let r = assert_call(&mut t, Value::Int(0), "assert(0)");
        let (class, msg) = thrown(r);
        assert_eq!(class, "ArgumentCountError");
        assert!(
            msg.starts_with("getmypid() expects exactly 0 arguments, 4 given"),
            "{msg}"
        );
    }

    #[test]
    fn constants_are_deprecated() {
        let t = TestVm::new();
        assert_eq!(t.vm.constant("ASSERT_EXCEPTION").unwrap().as_int(), Some(5));
        assert_eq!(t.vm.constant("ASSERT_BAIL").unwrap().as_int(), Some(3));
    }
}
