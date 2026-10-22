//! Helpers shared by the ext/date functions and classes: the module globals
//! (`DATEG`), the default timezone (`guess_timezone`, `get_timezone_info`),
//! the current time, parse-error messages and the property helpers of
//! php_date.c (`update_property`, `add_common_properties`).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use php_runtime::class::{ClassRef, member_flags};
use php_runtime::native::Args;
use php_runtime::types::CoerceMode;
use php_runtime::value::convert::object_property_table;
use php_runtime::{ArrayKey, Flow, ObjRef, PResult, PhpArray, PhpStr, Value, Vm};
use php_timelib::{ErrorContainer, TzInfo, builtin_tzinfo, timezone_id_is_valid};

pub const DATE_INTERFACE: &str = "DateTimeInterface";
pub const DATE_CLASS: &str = "DateTime";
pub const IMMUTABLE_CLASS: &str = "DateTimeImmutable";
pub const TIMEZONE_CLASS: &str = "DateTimeZone";
pub const INTERVAL_CLASS: &str = "DateInterval";
pub const PERIOD_CLASS: &str = "DatePeriod";

pub const DATE_ERROR: &str = "DateError";
pub const DATE_OBJECT_ERROR: &str = "DateObjectError";
pub const DATE_RANGE_ERROR: &str = "DateRangeError";
pub const DATE_EXCEPTION: &str = "DateException";
pub const INVALID_TIMEZONE_EXCEPTION: &str = "DateInvalidTimeZoneException";
pub const INVALID_OPERATION_EXCEPTION: &str = "DateInvalidOperationException";
pub const MALFORMED_STRING_EXCEPTION: &str = "DateMalformedStringException";
pub const MALFORMED_INTERVAL_STRING_EXCEPTION: &str = "DateMalformedIntervalStringException";
pub const MALFORMED_PERIOD_STRING_EXCEPTION: &str = "DateMalformedPeriodStringException";

/// The module globals (`zend_date_globals`).
#[derive(Default)]
pub struct DateGlobals {
    /// `DATEG(timezone)`: the zone set by `date_default_timezone_set()`.
    pub timezone: Option<String>,
    /// `DATEG(last_errors)`: warnings and errors of the last parse.
    pub last_errors: Option<ErrorContainer>,
}

pub fn globals(vm: &mut Vm) -> &mut DateGlobals {
    vm.ext_data::<DateGlobals>()
}

/// `guess_timezone`: `date_default_timezone_set()`, then `date.timezone`,
/// then UTC. (`OnUpdate_date_timezone` rejects invalid runtime values; a
/// startup value is checked here, like the `cfg_get_entry` branch.)
pub fn guess_timezone(vm: &mut Vm) -> String {
    if let Some(tz) = &globals(vm).timezone
        && !tz.is_empty()
    {
        return tz.clone();
    }
    if let Some(v) = vm.ini_get("date.timezone")
        && let Some(s) = v.as_str()
        && !s.is_empty()
        && timezone_id_is_valid(s)
    {
        return s.to_string();
    }
    "UTC".to_string()
}

/// `get_timezone_info`: the default timezone's data.
pub fn get_timezone_info(vm: &mut Vm) -> PResult<Arc<TzInfo>> {
    let name = guess_timezone(vm);
    match builtin_tzinfo(&name) {
        Some(tz) => Ok(tz),
        None => Err(vm.throw_error(
            DATE_ERROR,
            "Timezone database is corrupt. Please file a bug report as this should never happen",
        )),
    }
}

/// The current time as `(seconds, microseconds)` (`gettimeofday`).
pub fn current_time() -> (i64, i64) {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, i64::from(d.subsec_micros())),
        Err(e) => {
            let d = e.duration();
            let mut sec = -(d.as_secs() as i64);
            let mut usec = -i64::from(d.subsec_micros());
            if usec < 0 {
                sec -= 1;
                usec += 1_000_000;
            }
            (sec, usec)
        }
    }
}

/// `php_time()`.
pub fn php_time() -> i64 {
    current_time().0
}

/// The timezone lookup passed to the timelib parsers
/// (`php_date_parse_tzfile_wrapper`).
pub fn tz_get(name: &str) -> Option<Arc<TzInfo>> {
    builtin_tzinfo(name)
}

/// The part of `s` before the first NUL byte (how `%s` prints a C string).
pub fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&b| b == 0) {
        Some(i) => &s[..i],
        None => s,
    }
}

/// `"<prefix>(<string>) at position <n> (<c>)<infix>: <message>"` for the
/// first error of `err` (the common shape of php_date.c's parse errors).
pub fn first_error_message(
    prefix: &[u8],
    string: &[u8],
    infix: &[u8],
    err: &ErrorContainer,
) -> Vec<u8> {
    let mut out = prefix.to_vec();
    out.push(b'(');
    out.extend_from_slice(c_str(string));
    out.push(b')');
    if let Some(e) = err.errors.first() {
        out.extend_from_slice(format!(" at position {} (", e.position as i32).as_bytes());
        out.push(if e.character == 0 { b' ' } else { e.character });
        out.push(b')');
        out.extend_from_slice(infix);
        out.extend_from_slice(b": ");
        out.extend_from_slice(e.message.as_bytes());
    }
    out
}

/// `update_errors_warnings`: keeps the container only if it has warnings or
/// errors.
pub fn update_errors_warnings(vm: &mut Vm, err: &ErrorContainer) {
    let g = globals(vm);
    g.last_errors = if err.warning_count() > 0 || err.error_count() > 0 {
        Some(err.clone())
    } else {
        None
    };
}

/// `date_throw_uninitialized_error`.
pub fn uninitialized_error(vm: &mut Vm, ce: &ClassRef) -> Flow {
    let msg = if ce.is_internal() {
        format!(
            "Object of type {} has not been correctly initialized by calling parent::__construct() in its constructor",
            ce.name
        )
    } else {
        let mut ce_ptr = ce.clone();
        while !ce_ptr.is_internal()
            && let Some(p) = ce_ptr.parent.clone()
        {
            ce_ptr = p;
        }
        if !ce_ptr.is_internal() {
            format!(
                "Object of type {} not been correctly initialized by calling parent::__construct() in its constructor",
                ce.name
            )
        } else {
            format!(
                "Object of type {} (inheriting {}) has not been correctly initialized by calling parent::__construct() in its constructor",
                ce.name, ce_ptr.name
            )
        }
    };
    vm.throw_error(DATE_OBJECT_ERROR, msg)
}

/// A registered class of this module.
pub fn class(vm: &Vm, name: &str) -> ClassRef {
    vm.find_class(name.as_bytes())
        .unwrap_or_else(|| panic!("class {name} is registered by ext/date"))
}

/// `object_init_ex`: refuses abstract classes, interfaces, traits and enums.
pub fn object_init_ex(vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let what = if ce.is_interface() {
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
    if let Some(what) = what {
        return Err(vm.throw_error("Error", format!("Cannot instantiate {what} {}", ce.name)));
    }
    vm.instantiate(ce)
}

/// The class a static factory instantiates: the called scope
/// (`execute_data->This.value.ce`) or `default` when called as a function.
pub fn called_class(vm: &Vm, args: &Args, default: &str) -> ClassRef {
    match args.scope() {
        Some(ce) => ce.clone(),
        None => class(vm, default),
    }
}

/// `$this` of a native method.
pub fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("ext/date method called without $this")
}

/// Whether `ce` declares a (userland) `__debugInfo()`.
pub fn has_debug_info(ce: &ClassRef) -> bool {
    ce.magic.debug_info.is_some()
}

/// `Z_PARAM_PATH`: rejects strings with NUL bytes.
pub fn check_path(vm: &mut Vm, arg_num: usize, s: &[u8]) -> PResult<()> {
    if s.contains(&0) {
        return Err(vm.argument_value_error(arg_num, "must not contain any null bytes"));
    }
    Ok(())
}

/// `zend_hash_str_update` on a property table.
pub fn hash_update(props: &mut PhpArray, key: &str, v: impl Into<Value>) {
    props.insert(ArrayKey::Str(PhpStr::from(key)), v.into());
}

/// `add_common_properties`: the object's own properties that are not yet in
/// `table` (`zend_hash_add`).
pub fn add_common_properties(table: &mut PhpArray, obj: &ObjRef) {
    for (k, v) in object_property_table(obj).iter() {
        if !table.contains_key(&k) {
            table.insert(k, v.unref());
        }
    }
}

/// `zend_unmangle_property_name_ex`: `(class, name)` of a property-table key.
fn unmangle(key: &[u8]) -> Option<(Option<&[u8]>, &[u8])> {
    if key.first() != Some(&0) {
        return Some((None, key));
    }
    let rest = &key[1..];
    let end = rest.iter().position(|c| *c == 0)?;
    Some((Some(&rest[..end]), &rest[end + 1..]))
}

/// `update_property` (php_date.c) + `zend_update_property`: writes a
/// property restored by `__unserialize()`, honouring the class's declared
/// properties (type coercion) and the dynamic-property deprecation.
pub fn update_property(vm: &mut Vm, obj: &ObjRef, key: &PhpStr, value: &Value) -> PResult<()> {
    let Some((scope, name)) = unmangle(key.as_bytes()) else {
        return Ok(());
    };
    let class = obj.class().clone();
    if let Some(scope) = scope
        && scope != b"*"
    {
        // A private property: only written if its class exists.
        if vm.lookup_class(scope)?.is_none() {
            return Ok(());
        }
    }
    let value = value.unref();
    // DateInterval / DatePeriod route their virtual properties through the
    // write_property handler first.
    if let Some(h) = class.handlers.write_property
        && h(vm, obj, &PhpStr::from_bytes(name), value.clone())?
    {
        return Ok(());
    }
    if let Some(info) = class.find_property(name).filter(|p| !p.is_static()) {
        let declaring = info.declaring_class.clone();
        let ty = info.ty.clone();
        let mut v = value;
        if let Some(ty) = ty {
            match vm.coerce_to_type(&ty, v.clone(), CoerceMode::default())? {
                Some(c) => v = c,
                None => {
                    return Err(vm.throw_type_error(format!(
                        "Cannot assign {} to property {}::${} of type {}",
                        v.value_name(),
                        declaring,
                        String::from_utf8_lossy(name),
                        ty.to_type_string()
                    )));
                }
            }
        }
        obj.set_prop(name, v);
        return Ok(());
    }
    let exists = obj
        .obj()
        .dyn_props
        .borrow()
        .as_ref()
        .is_some_and(|d| d.get_str(name).is_some());
    if !exists && !class.has_flag(php_runtime::class::class_flags::ALLOW_DYNAMIC_PROPERTIES) {
        vm.engine_deprecated(format!(
            "Creation of dynamic property {}::${} is deprecated",
            class.name,
            String::from_utf8_lossy(name)
        ))?;
    }
    obj.set_prop(name, value);
    Ok(())
}

/// `restore_custom_*_properties`: every string-keyed, non-reference entry of
/// `data` except the class's internal ones.
pub fn restore_custom_properties(
    vm: &mut Vm,
    obj: &ObjRef,
    data: &PhpArray,
    is_internal: fn(&[u8]) -> bool,
) -> PResult<()> {
    for (k, v) in data.iter() {
        let ArrayKey::Str(name) = k else {
            continue;
        };
        if v.is_ref() || is_internal(name.as_bytes()) {
            continue;
        }
        update_property(vm, obj, &name, v)?;
    }
    Ok(())
}

/// `Z_OBJPROP_P(object)` for the `__wakeup()` methods: the object's own
/// property table.
pub fn object_props(obj: &ObjRef) -> PhpArray {
    object_property_table(obj)
}

/// `zend_deprecated_function` for the `#[\Deprecated]` methods.
pub fn deprecated_method(vm: &mut Vm, class: &str, method: &str, suffix: &str) -> PResult<()> {
    vm.engine_deprecated(format!("Method {class}::{method}() is deprecated{suffix}"))
}

/// The suffix of the 8.5 `__wakeup()` deprecations.
pub const WAKEUP_DEPRECATION: &str = " since 8.5, this method is obsolete, as serialization hooks are provided by __unserialize() and __serialize()";

/// `%g` as printed by `zend_argument_error` (php's spprintf).
pub fn format_g(d: f64) -> String {
    if d.is_nan() {
        "NAN".to_string()
    } else if d.is_infinite() {
        if d > 0.0 { "INF" } else { "-INF" }.to_string()
    } else {
        php_runtime::value::float_fmt::gcvt(d, 6, b'e')
    }
}

/// Whether a member (method, property) is public.
pub fn is_public(flags: u32) -> bool {
    flags & member_flags::PPP_MASK == member_flags::PUBLIC
}
