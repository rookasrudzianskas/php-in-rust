//! `DateTimeInterface`, `DateTime` and `DateTimeImmutable`, plus the
//! procedural `date_*` functions operating on them (`date_create`,
//! `date_format`, `date_modify`, `date_add`, ...), ported from php_date.c.

use php_runtime::class::{
    ClassConstant, ClassEntry, Function, PropPurpose, class_flags, fn_flags, member_flags,
};
use php_runtime::types::TypeDecl;
use php_runtime::value::convert::object_property_table;
use php_runtime::{
    Args, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};
use php_timelib::php::PhpTimeZone;
use php_timelib::{
    NO_CLONE, OVERRIDE_TIME, RelTime, Time, UNSET, ZoneType, add, add_wall, daynr_from_weeknr,
    diff, parse_from_format, strtotime, sub, sub_wall,
};

use super::common::{
    DATE_CLASS, DATE_INTERFACE, DATE_OBJECT_ERROR, DATE_RANGE_ERROR, IMMUTABLE_CLASS,
    INVALID_OPERATION_EXCEPTION, MALFORMED_STRING_EXCEPTION, WAKEUP_DEPRECATION,
    add_common_properties, c_str, called_class, check_path, class, current_time, deprecated_method,
    first_error_message, format_g, get_timezone_info, has_debug_info, hash_update, object_init_ex,
    object_props, restore_custom_properties, this, tz_get, uninitialized_error,
    update_errors_warnings,
};
use super::interval::{CIVIL, IntervalObj, WALL, new_interval_object};
use super::timezone::{new_timezone_object, timezone_of};

/// The state of a `DateTime` / `DateTimeImmutable` (`php_date_obj`).
#[derive(Clone, Debug, Default)]
pub struct DateObj {
    pub time: Option<Time>,
}

/// `PHP_DATE_INIT_CTOR`.
pub const INIT_CTOR: u32 = 0x01;
/// `PHP_DATE_INIT_FORMAT`.
pub const INIT_FORMAT: u32 = 0x02;

/// `date_object_new_date`.
pub fn create_date_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(DateObj::default());
    Ok(obj)
}

/// The time of a date object (`None` if uninitialised).
pub fn date_time(obj: &ObjRef) -> Option<Time> {
    obj.native::<DateObj>().and_then(|d| d.time.clone())
}

/// Replaces the time of a date object.
pub fn set_date_time(obj: &ObjRef, t: Option<Time>) {
    let done = match obj.native_mut::<DateObj>() {
        Some(mut d) => {
            d.time = t.clone();
            true
        }
        None => false,
    };
    if !done {
        obj.set_native(DateObj { time: t });
    }
}

/// `DATE_CHECK_INITIALIZED(dateobj->time, Z_OBJCE_P(object))`.
pub fn checked_time(vm: &mut Vm, obj: &ObjRef) -> PResult<Time> {
    match date_time(obj) {
        Some(t) => Ok(t),
        None => {
            let ce = obj.class().clone();
            Err(uninitialized_error(vm, &ce))
        }
    }
}

/// Runs `f` on the (initialised) time of `obj`.
fn update_time<R>(vm: &mut Vm, obj: &ObjRef, f: impl FnOnce(&mut Time) -> R) -> PResult<R> {
    let mut t = checked_time(vm, obj)?;
    let r = f(&mut t);
    set_date_time(obj, Some(t));
    Ok(r)
}

/// `get_base_date_class`: `DateTime` or `DateTimeImmutable` for a subclass.
pub fn base_date_class(ce: &ClassRef) -> ClassRef {
    let mut tmp = ce.clone();
    loop {
        let lc = tmp.lc_name.as_bytes();
        if lc == b"datetime" || lc == b"datetimeimmutable" {
            return tmp;
        }
        match tmp.parent.clone() {
            Some(p) => tmp = p,
            None => return tmp,
        }
    }
}

/// A new date object of class `ce` holding `time`.
pub fn new_date_object(vm: &mut Vm, ce: &ClassRef, time: Time) -> PResult<ObjRef> {
    let obj = object_init_ex(vm, ce)?;
    set_date_time(&obj, Some(time));
    Ok(obj)
}

/// `date_object_clone_date`: the members and the time. A userland
/// `__clone()` is left to the engine (see [`clone_immutable`]).
pub fn clone_date_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = create_date_object(vm, &ce)?;
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    set_date_time(&new, date_time(old));
    Ok(new)
}

/// `date_clone_immutable`: the clone the `DateTimeImmutable` modifiers work
/// on, calling a userland `__clone()` like `zend_objects_clone_members`.
fn clone_immutable(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    let new = clone_date_object(vm, obj)?;
    if obj
        .class()
        .magic
        .clone
        .as_ref()
        .is_some_and(|m| !m.is_native())
    {
        vm.call_method(&new, b"__clone", Vec::new())?;
    }
    Ok(new)
}

/// `ZEND_COMPARE_OBJECTS_FALLBACK`: whether both operands are objects with
/// the same compare handler.
pub fn same_compare_handler(a: &Value, b: &Value) -> Option<(ObjRef, ObjRef)> {
    let (Value::Object(o1), Value::Object(o2)) = (a, b) else {
        return None;
    };
    match (o1.class().handlers.compare, o2.class().handlers.compare) {
        (Some(x), Some(y)) if std::ptr::fn_addr_eq(x, y) => Some((o1.clone(), o2.clone())),
        _ => None,
    }
}

/// `date_object_compare_date`.
fn compare_dates(vm: &mut Vm, a: &Value, b: &Value) -> PResult<i32> {
    let Some((o1, o2)) = same_compare_handler(a, b) else {
        return vm.std_compare_objects(a, b);
    };
    let (Some(mut t1), Some(mut t2)) = (date_time(&o1), date_time(&o2)) else {
        return Err(vm.throw_error(
            DATE_OBJECT_ERROR,
            "Trying to compare an incomplete DateTime or DateTimeImmutable object",
        ));
    };
    if !t1.sse_uptodate {
        let tz = t1.tz_info.clone();
        t1.update_ts(tz.as_ref());
        set_date_time(&o1, Some(t1.clone()));
    }
    if !t2.sse_uptodate {
        let tz = t2.tz_info.clone();
        t2.update_ts(tz.as_ref());
        set_date_time(&o2, Some(t2.clone()));
    }
    Ok(t1.compare(&t2) as i32)
}

/// `date_create_tz_offset_str`.
pub fn tz_offset_str(offset: i64) -> String {
    php_timelib::format::tz_offset_str(offset)
}

/// `date_object_to_hash`.
pub fn date_object_to_hash(time: &Time, props: &mut PhpArray) {
    let date = php_timelib::date_format(b"x-m-d H:i:s.u", time, true);
    hash_update(props, "date", Value::from(date));
    if time.is_localtime {
        hash_update(props, "timezone_type", i64::from(time.zone_type.as_i32()));
        let tz = match time.zone_type {
            ZoneType::Id => Value::str(time.tz_name().unwrap_or("")),
            ZoneType::Offset => Value::from(tz_offset_str(i64::from(time.z))),
            ZoneType::Abbr => Value::str(time.tz_abbr.as_deref().unwrap_or("")),
            ZoneType::None => Value::Null,
        };
        hash_update(props, "timezone", tz);
    }
}

/// `date_object_get_properties_for`.
fn date_properties_for(
    _vm: &mut Vm,
    obj: &ObjRef,
    purpose: PropPurpose,
) -> PResult<Option<PhpArray>> {
    match purpose {
        PropPurpose::Serialize
        | PropPurpose::VarExport
        | PropPurpose::Json
        | PropPurpose::ArrayCast => {}
        PropPurpose::Debug => {
            if has_debug_info(obj.class()) {
                return Ok(None);
            }
        }
        PropPurpose::GetObjectVars => return Ok(None),
    }
    let mut props = object_property_table(obj);
    if let Some(t) = date_time(obj) {
        date_object_to_hash(&t, &mut props);
    }
    Ok(Some(props))
}

/// `php_date_initialize`: parses `time_str` (with `format` for
/// `createFromFormat`) into `obj`. Returns `false` on parse errors (after
/// throwing for constructors).
pub fn php_date_initialize(
    vm: &mut Vm,
    obj: &ObjRef,
    time_str: &[u8],
    format: Option<&[u8]>,
    timezone_object: Option<&ObjRef>,
    flags: u32,
) -> PResult<bool> {
    set_date_time(obj, None);
    let time_str: &[u8] = if format.is_none() && time_str.is_empty() {
        b"now"
    } else {
        time_str
    };
    let res = match format {
        Some(f) => parse_from_format(f, time_str, &tz_get),
        None => strtotime(time_str, &tz_get),
    };
    update_errors_warnings(vm, &res.errors);
    if res.errors.error_count() > 0 {
        if flags & INIT_CTOR != 0 {
            // spit out the first library error message, at least
            let msg =
                first_error_message(b"Failed to parse time string ", time_str, b"", &res.errors);
            return Err(vm.throw_error(MALFORMED_STRING_EXCEPTION, PhpStr::from_vec(msg)));
        }
        return Ok(false);
    }
    let mut time = res.time;

    let zone = match timezone_object {
        Some(tzobj) => match timezone_of(tzobj) {
            Some(z) => z,
            None => {
                return Err(vm.throw_error(
                    "Error",
                    "The DateTimeZone object has not been correctly initialized by its constructor",
                ));
            }
        },
        None => match &time.tz_info {
            Some(tz) => PhpTimeZone::Id(tz.clone()),
            None => PhpTimeZone::Id(get_timezone_info(vm)?),
        },
    };
    let tzi = match &zone {
        PhpTimeZone::Id(tz) => Some(tz.clone()),
        _ => None,
    };

    let mut now = Time {
        zone_type: zone.zone_type(),
        ..Time::new()
    };
    match &zone {
        PhpTimeZone::Id(tz) => now.tz_info = Some(tz.clone()),
        PhpTimeZone::Offset(o) => now.z = *o as i32,
        PhpTimeZone::Abbr(a) => {
            now.z = a.utc_offset as i32;
            now.dst = a.dst;
            now.tz_abbr = Some(a.abbr.clone());
        }
    }
    let (sec, usec) = current_time();
    now.unixtime2local(sec);
    now.us = usec;

    if format.is_none() && time_str == b"now" {
        set_date_time(obj, Some(now));
        return Ok(true);
    }

    let mut options = NO_CLONE;
    if flags & INIT_FORMAT != 0 {
        options |= OVERRIDE_TIME;
    }
    time.fill_holes(&now, options);
    time.update_ts(tzi.as_ref());
    time.update_from_sse();
    time.have_relative = false;
    set_date_time(obj, Some(time));
    Ok(true)
}

/// `php_date_initialize_from_ts_long`.
fn time_from_ts_long(sec: i64, usec: i64) -> Time {
    let mut t = Time {
        zone_type: ZoneType::Offset,
        ..Time::new()
    };
    t.unixtime2gmt(sec);
    t.update_ts(None);
    t.us = usec;
    t
}

/// `php_date_initialize_from_ts_double`.
fn time_from_ts_double(vm: &mut Vm, ts: f64) -> PResult<Time> {
    let range_error = |vm: &mut Vm| {
        vm.argument_error(
            DATE_RANGE_ERROR,
            1,
            &format!(
                "must be a finite number between {} and {}.999999, {} given",
                i64::MIN,
                i64::MAX,
                format_g(ts)
            ),
        )
    };
    let sec_dval = ts.trunc();
    // >= as (double)TIMELIB_LONG_MAX is outside signed range
    if sec_dval.is_nan() || sec_dval >= i64::MAX as f64 || sec_dval < i64::MIN as f64 {
        return Err(range_error(vm));
    }
    let mut sec = sec_dval as i64;
    let mut usec = ((ts % 1.0) * 1_000_000.0).round() as i64;
    if usec.abs() == 1_000_000 {
        sec += if usec > 0 { 1 } else { -1 };
        usec = 0;
    }
    if usec < 0 {
        if sec == i64::MIN {
            return Err(range_error(vm));
        }
        sec -= 1;
        usec += 1_000_000;
    }
    Ok(time_from_ts_long(sec, usec))
}

// ---------------------------------------------------------------------------
// Shared implementations (function and method forms)
// ---------------------------------------------------------------------------

/// The date object of an alias handler: `$this` for methods, the first
/// argument for the procedural form, plus the offset of the remaining
/// arguments (`zend_parse_method_parameters`).
pub fn target(args: &Args) -> (ObjRef, usize) {
    match args.this() {
        Some(t) => (t.clone(), 0),
        None => (args.object(0).clone(), 1),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModifyMode {
    Warning,
    Throw,
}

/// `php_date_modify`.
fn php_date_modify(vm: &mut Vm, obj: &ObjRef, modify: &[u8], mode: ModifyMode) -> PResult<bool> {
    let Some(mut t) = date_time(obj) else {
        let ce = obj.class().clone();
        return Err(uninitialized_error(vm, &ce));
    };
    let res = strtotime(modify, &tz_get);
    update_errors_warnings(vm, &res.errors);
    if res.errors.error_count() > 0 {
        if mode == ModifyMode::Throw {
            let prefix = format!(
                "{}(): Failed to parse time string ",
                vm.current_function_display_name()
            );
            let msg = first_error_message(prefix.as_bytes(), modify, b"", &res.errors);
            return Err(vm.throw_error(MALFORMED_STRING_EXCEPTION, PhpStr::from_vec(msg)));
        }
        let msg = first_error_message(b"Failed to parse time string ", modify, b"", &res.errors);
        vm.warning(PhpStr::from_vec(msg))?;
        return Ok(false);
    }
    let tmp = res.time;
    t.relative = tmp.relative;
    t.have_relative = tmp.have_relative;
    t.sse_uptodate = false;
    if tmp.y != UNSET {
        t.y = tmp.y;
    }
    if tmp.m != UNSET {
        t.m = tmp.m;
    }
    if tmp.d != UNSET {
        t.d = tmp.d;
    }
    if tmp.h != UNSET {
        t.h = tmp.h;
        if tmp.i != UNSET {
            t.i = tmp.i;
            t.s = if tmp.s != UNSET { tmp.s } else { 0 };
        } else {
            t.i = 0;
            t.s = 0;
        }
    }
    if tmp.us != UNSET {
        t.us = tmp.us;
    }
    // Reset timezone to UTC if we detect a "@<ts>" modification
    if tmp.y == 1970
        && tmp.m == 1
        && tmp.d == 1
        && tmp.h == 0
        && tmp.i == 0
        && tmp.s == 0
        && tmp.us == 0
        && tmp.have_zone != 0
        && tmp.zone_type == ZoneType::Offset
        && tmp.z == 0
        && tmp.dst == 0
    {
        t.set_timezone_from_offset(0);
    }
    t.update_ts(None);
    t.update_from_sse();
    t.have_relative = false;
    t.relative = RelTime::default();
    set_date_time(obj, Some(t));
    Ok(true)
}

/// The interval of a `DateInterval` argument (`DATE_CHECK_INITIALIZED`).
fn checked_interval(vm: &mut Vm, interval: &ObjRef) -> PResult<IntervalObj> {
    let state = interval.native::<IntervalObj>().map(|i| i.clone());
    match state {
        Some(i) if i.initialized => Ok(i),
        _ => {
            let ce = interval.class().clone();
            Err(uninitialized_error(vm, &ce))
        }
    }
}

/// `php_date_add`.
fn php_date_add(vm: &mut Vm, obj: &ObjRef, interval: &ObjRef) -> PResult<()> {
    let t = checked_time(vm, obj)?;
    let intobj = checked_interval(vm, interval)?;
    let diff = intobj.diff.unwrap_or_default();
    let new_time = if intobj.civil_or_wall == WALL {
        add_wall(&t, &diff)
    } else {
        add(&t, &diff)
    };
    set_date_time(obj, Some(new_time));
    Ok(())
}

/// `php_date_sub`.
fn php_date_sub(vm: &mut Vm, obj: &ObjRef, interval: &ObjRef, should_throw: bool) -> PResult<()> {
    let t = checked_time(vm, obj)?;
    let intobj = checked_interval(vm, interval)?;
    let diff = intobj.diff.unwrap_or_default();
    if diff.have_weekday_relative || diff.have_special_relative {
        if should_throw {
            let msg = format!(
                "{}(): Only non-special relative time specifications are supported for subtraction",
                vm.current_function_display_name()
            );
            return Err(vm.throw_error(INVALID_OPERATION_EXCEPTION, msg));
        }
        vm.warning("Only non-special relative time specifications are supported for subtraction")?;
        return Ok(());
    }
    let new_time = if intobj.civil_or_wall == WALL {
        sub_wall(&t, &diff)
    } else {
        sub(&t, &diff)
    };
    set_date_time(obj, Some(new_time));
    Ok(())
}

/// `php_date_timezone_set`.
fn php_date_timezone_set(vm: &mut Vm, obj: &ObjRef, tzobj: &ObjRef) -> PResult<()> {
    let mut t = checked_time(vm, obj)?;
    match timezone_of(tzobj) {
        Some(PhpTimeZone::Offset(o)) => t.set_timezone_from_offset(o),
        Some(PhpTimeZone::Abbr(a)) => t.set_timezone_from_abbr(&a),
        Some(PhpTimeZone::Id(tz)) => t.set_timezone(&tz),
        None => {}
    }
    let sse = t.sse;
    t.unixtime2local(sse);
    set_date_time(obj, Some(t));
    Ok(())
}

/// `php_date_time_set`.
fn php_date_time_set(vm: &mut Vm, obj: &ObjRef, h: i64, i: i64, s: i64, us: i64) -> PResult<()> {
    update_time(vm, obj, |t| {
        t.h = h;
        t.i = i;
        t.s = s;
        t.us = us;
        t.update_ts(None);
        t.update_from_sse();
    })
}

/// `php_date_date_set`.
fn php_date_date_set(vm: &mut Vm, obj: &ObjRef, y: i64, m: i64, d: i64) -> PResult<()> {
    update_time(vm, obj, |t| {
        t.y = y;
        t.m = m;
        t.d = d;
        t.update_ts(None);
    })
}

/// `php_date_isodate_set`.
fn php_date_isodate_set(vm: &mut Vm, obj: &ObjRef, y: i64, w: i64, d: i64) -> PResult<()> {
    update_time(vm, obj, |t| {
        t.y = y;
        t.m = 1;
        t.d = 1;
        t.relative = RelTime::default();
        t.relative.d = daynr_from_weeknr(y, w, d);
        t.have_relative = true;
        t.update_ts(None);
    })
}

/// `php_date_timestamp_set`.
fn php_date_timestamp_set(vm: &mut Vm, obj: &ObjRef, ts: i64) -> PResult<()> {
    update_time(vm, obj, |t| {
        t.unixtime2local(ts);
        t.update_ts(None);
        t.us = 0;
    })
}

fn check_microsecond(vm: &mut Vm, us: i64) -> PResult<()> {
    if !(0..=999_999).contains(&us) {
        return Err(vm.argument_error(
            DATE_RANGE_ERROR,
            1,
            &format!("must be between 0 and 999999, {us} given"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Procedural functions and their method aliases
// ---------------------------------------------------------------------------

/// `date_create()` / `date_create_immutable()`.
fn date_create_common(vm: &mut Vm, args: &Args, ce_name: &str) -> PResult<Value> {
    let ce = class(vm, ce_name);
    let obj = vm.instantiate(&ce)?;
    let tz = args.object_opt(1).cloned();
    if !php_date_initialize(vm, &obj, args.str(0).as_bytes(), None, tz.as_ref(), 0)? {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Object(obj))
}

fn date_create(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    date_create_common(vm, args, DATE_CLASS)
}

fn date_create_immutable(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    date_create_common(vm, args, IMMUTABLE_CLASS)
}

/// `date_create_from_format()` / `date_create_immutable_from_format()` and
/// the `createFromFormat()` methods.
fn create_from_format_common(vm: &mut Vm, args: &Args, default: &str) -> PResult<Value> {
    check_path(vm, 2, args.str(1).as_bytes())?;
    let ce = called_class(vm, args, default);
    let obj = object_init_ex(vm, &ce)?;
    let tz = args.object_opt(2).cloned();
    let format = args.str(0).clone();
    if !php_date_initialize(
        vm,
        &obj,
        args.str(1).as_bytes(),
        Some(format.as_bytes()),
        tz.as_ref(),
        INIT_FORMAT,
    )? {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Object(obj))
}

fn date_create_from_format(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    create_from_format_common(vm, args, DATE_CLASS)
}

fn date_create_immutable_from_format(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    create_from_format_common(vm, args, IMMUTABLE_CLASS)
}

/// `date_format()` / `DateTimeInterface::format()`.
fn date_format_fn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let t = checked_time(vm, &obj)?;
    let format = args.str(off).clone();
    Ok(Value::from(php_timelib::date_format(
        format.as_bytes(),
        &t,
        t.is_localtime,
    )))
}

fn date_modify(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    let modify = args.str(1).clone();
    if !php_date_modify(vm, &obj, modify.as_bytes(), ModifyMode::Warning)? {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Object(obj))
}

/// `date_add()` / `DateTime::add()`.
fn date_add_fn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let interval = args.object(off).clone();
    php_date_add(vm, &obj, &interval)?;
    Ok(Value::Object(obj))
}

fn date_sub_fn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    let interval = args.object(1).clone();
    php_date_sub(vm, &obj, &interval, false)?;
    Ok(Value::Object(obj))
}

/// `date_timezone_get()` / `getTimezone()`.
fn date_timezone_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, _) = target(args);
    let t = checked_time(vm, &obj)?;
    if !t.is_localtime {
        return Ok(Value::Bool(false));
    }
    match PhpTimeZone::from_time(&t) {
        Some(zone) => Ok(Value::Object(new_timezone_object(vm, zone)?)),
        None => Ok(Value::Bool(false)),
    }
}

/// `date_timezone_set()` / `DateTime::setTimezone()`.
fn date_timezone_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let tz = args.object(off).clone();
    php_date_timezone_set(vm, &obj, &tz)?;
    Ok(Value::Object(obj))
}

/// `date_offset_get()` / `getOffset()`.
fn date_offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, _) = target(args);
    let t = checked_time(vm, &obj)?;
    if !t.is_localtime {
        return Ok(Value::Int(0));
    }
    Ok(match t.zone_type {
        ZoneType::Id => Value::Int(
            t.tz_info
                .as_deref()
                .map_or(0, |tz| i64::from(tz.get_time_zone_info(t.sse).offset)),
        ),
        ZoneType::Offset => Value::Int(i64::from(t.z)),
        ZoneType::Abbr => Value::Int(i64::from(t.z) + 3600 * i64::from(t.dst)),
        ZoneType::None => Value::Null,
    })
}

/// `date_time_set()` / `DateTime::setTime()`.
fn date_time_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let (h, i, s, us) = (
        args.int(off),
        args.int(off + 1),
        args.int(off + 2),
        args.int(off + 3),
    );
    php_date_time_set(vm, &obj, h, i, s, us)?;
    Ok(Value::Object(obj))
}

/// `date_date_set()` / `DateTime::setDate()`.
fn date_date_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let (y, m, d) = (args.int(off), args.int(off + 1), args.int(off + 2));
    php_date_date_set(vm, &obj, y, m, d)?;
    Ok(Value::Object(obj))
}

/// `date_isodate_set()` / `DateTime::setISODate()`.
fn date_isodate_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let (y, w, d) = (args.int(off), args.int(off + 1), args.int(off + 2));
    php_date_isodate_set(vm, &obj, y, w, d)?;
    Ok(Value::Object(obj))
}

/// `date_timestamp_set()` / `DateTime::setTimestamp()`.
fn date_timestamp_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let ts = args.int(off);
    php_date_timestamp_set(vm, &obj, ts)?;
    Ok(Value::Object(obj))
}

/// `date_timestamp_get()` / `getTimestamp()`.
fn date_timestamp_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, _) = target(args);
    let mut t = checked_time(vm, &obj)?;
    if !t.sse_uptodate {
        t.update_ts(None);
        set_date_time(&obj, Some(t.clone()));
    }
    Ok(Value::Int(t.sse))
}

/// `date_diff()` / `diff()`.
fn date_diff(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj1, off) = target(args);
    let obj2 = args.object(off).clone();
    let absolute = args.bool(off + 1);
    let t1 = checked_time(vm, &obj1)?;
    let t2 = checked_time(vm, &obj2)?;
    let mut d = diff(&t1, &t2);
    if absolute {
        d.invert = false;
    }
    Ok(Value::Object(new_interval_object(vm, d, CIVIL, None)?))
}

/// `DateTime::getLastErrors()` (an alias of `date_get_last_errors()`).
fn get_last_errors(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(super::functions::last_errors_value(vm))
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let tz = args.object_opt(1).cloned();
    php_date_initialize(
        vm,
        &obj,
        args.str(0).as_bytes(),
        None,
        tz.as_ref(),
        INIT_CTOR,
    )?;
    Ok(Value::Null)
}

/// The declaring class of the running method (`DateTime` or
/// `DateTimeImmutable`).
fn scope_name(args: &Args) -> &'static str {
    match args.function().scope.as_ref() {
        Some(s) if s.eq_ignore_ascii_case(IMMUTABLE_CLASS.as_bytes()) => IMMUTABLE_CLASS,
        _ => DATE_CLASS,
    }
}

/// `createFromImmutable()` / `createFromMutable()` / `createFromInterface()`.
fn create_from_object(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let old = args.object(0).clone();
    let time = checked_time(vm, &old)?;
    let ce = called_class(vm, args, scope_name(args));
    Ok(Value::Object(new_date_object(vm, &ce, time)?))
}

fn create_from_format_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    create_from_format_common(vm, args, scope_name(args))
}

/// `createFromTimestamp()`.
fn create_from_timestamp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ce = called_class(vm, args, scope_name(args));
    let obj = object_init_ex(vm, &ce)?;
    let time = match args.get(0) {
        Value::Float(d) => time_from_ts_double(vm, *d)?,
        v => time_from_ts_long(v.as_int().unwrap_or(0), 0),
    };
    set_date_time(&obj, Some(time));
    Ok(Value::Object(obj))
}

/// `php_date_initialize_from_hash`.
pub fn date_initialize_from_hash(vm: &mut Vm, obj: &ObjRef, myht: &PhpArray) -> PResult<bool> {
    let Some(Value::String(z_date)) = myht.get_str(b"date").map(Value::unref) else {
        return Ok(false);
    };
    let Some(Value::Int(tz_type)) = myht.get_str(b"timezone_type").map(Value::unref) else {
        return Ok(false);
    };
    let Some(Value::String(z_timezone)) = myht.get_str(b"timezone").map(Value::unref) else {
        return Ok(false);
    };
    match tz_type {
        1 | 2 => {
            let mut s = z_date.to_vec();
            s.push(b' ');
            s.extend_from_slice(&z_timezone);
            php_date_initialize(vm, obj, &s, None, None, 0)
        }
        3 => {
            let name = String::from_utf8_lossy(c_str(&z_timezone)).into_owned();
            let Some(tzi) = tz_get(&name) else {
                return Ok(false);
            };
            let tzobj = new_timezone_object(vm, PhpTimeZone::Id(tzi))?;
            php_date_initialize(vm, obj, &z_date, None, Some(&tzobj), 0)
        }
        _ => Ok(false),
    }
}

fn invalid_serialization(vm: &mut Vm, name: &str) -> php_runtime::Flow {
    vm.throw_error(
        "Error",
        format!("Invalid serialization data for {name} object"),
    )
}

fn set_state(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = scope_name(args);
    let ce = class(vm, name);
    let obj = vm.instantiate(&ce)?;
    let data = args.array(0).clone();
    if !date_initialize_from_hash(vm, &obj, &data)? {
        return Err(invalid_serialization(vm, name));
    }
    Ok(Value::Object(obj))
}

fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let t = checked_time(vm, &obj)?;
    let mut a = PhpArray::new();
    date_object_to_hash(&t, &mut a);
    add_common_properties(&mut a, &obj);
    Ok(Value::array(a))
}

/// `date_time_is_internal_property`.
fn is_internal_property(name: &[u8]) -> bool {
    matches!(name, b"date" | b"timezone_type" | b"timezone")
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let data = args.array(0).clone();
    if !date_initialize_from_hash(vm, &obj, &data)? {
        return Err(invalid_serialization(vm, scope_name(args)));
    }
    restore_custom_properties(vm, &obj, &data, is_internal_property)?;
    Ok(Value::Null)
}

fn wakeup(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = scope_name(args);
    deprecated_method(vm, name, "__wakeup", WAKEUP_DEPRECATION)?;
    let obj = this(args);
    let props = object_props(&obj);
    if !date_initialize_from_hash(vm, &obj, &props)? {
        return Err(invalid_serialization(vm, name));
    }
    Ok(Value::Null)
}

fn dt_modify(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let modify = args.str(0).clone();
    php_date_modify(vm, &obj, modify.as_bytes(), ModifyMode::Throw)?;
    Ok(Value::Object(obj))
}

fn dt_sub(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let interval = args.object(0).clone();
    php_date_sub(vm, &obj, &interval, true)?;
    Ok(Value::Object(obj))
}

fn get_microsecond(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let t = checked_time(vm, &obj)?;
    Ok(Value::Int(t.us))
}

fn dt_set_microsecond(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let us = args.int(0);
    check_microsecond(vm, us)?;
    let obj = this(args);
    update_time(vm, &obj, |t| t.us = us)?;
    Ok(Value::Object(obj))
}

fn dti_modify(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let modify = args.str(0).clone();
    let new = clone_immutable(vm, &obj)?;
    php_date_modify(vm, &new, modify.as_bytes(), ModifyMode::Throw)?;
    Ok(Value::Object(new))
}

fn dti_add(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let interval = args.object(0).clone();
    let new = clone_immutable(vm, &obj)?;
    php_date_add(vm, &new, &interval)?;
    Ok(Value::Object(new))
}

fn dti_sub(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let interval = args.object(0).clone();
    let new = clone_immutable(vm, &obj)?;
    php_date_sub(vm, &new, &interval, true)?;
    Ok(Value::Object(new))
}

fn dti_set_timezone(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let tz = args.object(0).clone();
    let new = clone_immutable(vm, &obj)?;
    php_date_timezone_set(vm, &new, &tz)?;
    Ok(Value::Object(new))
}

fn dti_set_time(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let (h, i, s, us) = (args.int(0), args.int(1), args.int(2), args.int(3));
    let new = clone_immutable(vm, &obj)?;
    php_date_time_set(vm, &new, h, i, s, us)?;
    Ok(Value::Object(new))
}

fn dti_set_date(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let (y, m, d) = (args.int(0), args.int(1), args.int(2));
    let new = clone_immutable(vm, &obj)?;
    php_date_date_set(vm, &new, y, m, d)?;
    Ok(Value::Object(new))
}

fn dti_set_isodate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let (y, w, d) = (args.int(0), args.int(1), args.int(2));
    let new = clone_immutable(vm, &obj)?;
    php_date_isodate_set(vm, &new, y, w, d)?;
    Ok(Value::Object(new))
}

fn dti_set_timestamp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let ts = args.int(0);
    let new = clone_immutable(vm, &obj)?;
    php_date_timestamp_set(vm, &new, ts)?;
    Ok(Value::Object(new))
}

fn dti_set_microsecond(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let us = args.int(0);
    check_microsecond(vm, us)?;
    let obj = this(args);
    checked_time(vm, &obj)?;
    let new = clone_immutable(vm, &obj)?;
    update_time(vm, &new, |t| t.us = us)?;
    Ok(Value::Object(new))
}

// ---------------------------------------------------------------------------
// Signatures and registration
// ---------------------------------------------------------------------------

macro_rules! nf {
    ($name:ident, $sig:expr, $handler:expr) => {
        pub static $name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}
pub(crate) use nf;

nf!(
    DATE_CREATE,
    "function date_create(string $datetime = \"now\", ?DateTimeZone $timezone = null): DateTime|false {}",
    date_create
);
nf!(
    DATE_CREATE_IMMUTABLE,
    "function date_create_immutable(
    string $datetime = \"now\", ?DateTimeZone $timezone = null): DateTimeImmutable|false {}",
    date_create_immutable
);
nf!(
    DATE_CREATE_FROM_FORMAT,
    "function date_create_from_format(
    string $format, string $datetime, ?DateTimeZone $timezone = null): DateTime|false {}",
    date_create_from_format
);
nf!(
    DATE_CREATE_IMMUTABLE_FROM_FORMAT,
    "function date_create_immutable_from_format(
    string $format, string $datetime, ?DateTimeZone $timezone = null): DateTimeImmutable|false {}",
    date_create_immutable_from_format
);
nf!(
    DATE_FORMAT,
    "function date_format(DateTimeInterface $object, string $format): string {}",
    date_format_fn
);
nf!(
    DATE_MODIFY,
    "function date_modify(DateTime $object, string $modifier): DateTime|false {}",
    date_modify
);
nf!(
    DATE_ADD,
    "function date_add(DateTime $object, DateInterval $interval): DateTime {}",
    date_add_fn
);
nf!(
    DATE_SUB,
    "function date_sub(DateTime $object, DateInterval $interval): DateTime {}",
    date_sub_fn
);
nf!(
    DATE_TIMEZONE_GET,
    "function date_timezone_get(DateTimeInterface $object): DateTimeZone|false {}",
    date_timezone_get
);
nf!(
    DATE_TIMEZONE_SET,
    "function date_timezone_set(DateTime $object, DateTimeZone $timezone): DateTime {}",
    date_timezone_set
);
nf!(
    DATE_OFFSET_GET,
    "function date_offset_get(DateTimeInterface $object): int {}",
    date_offset_get
);
nf!(
    DATE_DIFF,
    "function date_diff(
    DateTimeInterface $baseObject, DateTimeInterface $targetObject, bool $absolute = false): DateInterval {}",
    date_diff
);
nf!(
    DATE_TIME_SET,
    "function date_time_set(
    DateTime $object, int $hour, int $minute, int $second = 0, int $microsecond = 0): DateTime {}",
    date_time_set
);
nf!(
    DATE_DATE_SET,
    "function date_date_set(DateTime $object, int $year, int $month, int $day): DateTime {}",
    date_date_set
);
nf!(
    DATE_ISODATE_SET,
    "function date_isodate_set(DateTime $object, int $year, int $week, int $dayOfWeek = 1): DateTime {}",
    date_isodate_set
);
nf!(
    DATE_TIMESTAMP_SET,
    "function date_timestamp_set(DateTime $object, int $timestamp): DateTime {}",
    date_timestamp_set
);
nf!(
    DATE_TIMESTAMP_GET,
    "function date_timestamp_get(DateTimeInterface $object): int {}",
    date_timestamp_get
);

// DateTimeInterface
nf!(
    I_FORMAT,
    "public function format(string $format): string",
    date_format_fn
);
nf!(
    I_GET_TIMEZONE,
    "public function getTimezone(): DateTimeZone|false",
    date_timezone_get
);
nf!(
    I_GET_OFFSET,
    "public function getOffset(): int",
    date_offset_get
);
nf!(
    I_GET_TIMESTAMP,
    "public function getTimestamp(): int",
    date_timestamp_get
);
nf!(
    I_GET_MICROSECOND,
    "public function getMicrosecond(): int",
    get_microsecond
);
nf!(
    I_DIFF,
    "public function diff(DateTimeInterface $targetObject, bool $absolute = false): DateInterval",
    date_diff
);
nf!(I_WAKEUP, "public function __wakeup(): void", wakeup);
nf!(
    I_SERIALIZE,
    "public function __serialize(): array",
    serialize
);
nf!(
    I_UNSERIALIZE,
    "public function __unserialize(array $data): void",
    unserialize
);

// DateTime
nf!(
    DT_CONSTRUCT,
    "public function __construct(string $datetime = \"now\", ?DateTimeZone $timezone = null) {}",
    construct
);
nf!(
    DT_SERIALIZE,
    "public function __serialize(): array {}",
    serialize
);
nf!(
    DT_UNSERIALIZE,
    "public function __unserialize(array $data): void {}",
    unserialize
);
nf!(DT_WAKEUP, "public function __wakeup(): void {}", wakeup);
nf!(
    DT_SET_STATE,
    "public static function __set_state(array $array): DateTime {}",
    set_state
);
nf!(
    DT_CREATE_FROM_IMMUTABLE,
    "public static function createFromImmutable(DateTimeImmutable $object): static {}",
    create_from_object
);
nf!(
    DT_CREATE_FROM_INTERFACE,
    "public static function createFromInterface(DateTimeInterface $object): static {}",
    create_from_object
);
nf!(
    DT_CREATE_FROM_FORMAT,
    "public static function createFromFormat(string $format, string $datetime, ?DateTimeZone $timezone = null): DateTime|false {}",
    create_from_format_method
);
nf!(
    DT_CREATE_FROM_TIMESTAMP,
    "public static function createFromTimestamp(int|float $timestamp): static {}",
    create_from_timestamp
);
nf!(
    DT_GET_LAST_ERRORS,
    "public static function getLastErrors(): array|false {}",
    get_last_errors
);
nf!(
    DT_FORMAT,
    "public function format(string $format): string {}",
    date_format_fn
);
nf!(
    DT_MODIFY,
    "public function modify(string $modifier): DateTime {}",
    dt_modify
);
nf!(
    DT_ADD,
    "public function add(DateInterval $interval): DateTime {}",
    date_add_fn
);
nf!(
    DT_SUB,
    "public function sub(DateInterval $interval): DateTime {}",
    dt_sub
);
nf!(
    DT_GET_TIMEZONE,
    "public function getTimezone(): DateTimeZone|false {}",
    date_timezone_get
);
nf!(
    DT_SET_TIMEZONE,
    "public function setTimezone(DateTimeZone $timezone): DateTime {}",
    date_timezone_set
);
nf!(
    DT_GET_OFFSET,
    "public function getOffset(): int {}",
    date_offset_get
);
nf!(
    DT_GET_MICROSECOND,
    "public function getMicrosecond(): int {}",
    get_microsecond
);
nf!(
    DT_SET_TIME,
    "public function setTime(int $hour, int $minute, int $second = 0, int $microsecond = 0): DateTime {}",
    date_time_set
);
nf!(
    DT_SET_DATE,
    "public function setDate(int $year, int $month, int $day): DateTime {}",
    date_date_set
);
nf!(
    DT_SET_ISODATE,
    "public function setISODate(int $year, int $week, int $dayOfWeek = 1): DateTime {}",
    date_isodate_set
);
nf!(
    DT_SET_TIMESTAMP,
    "public function setTimestamp(int $timestamp): DateTime {}",
    date_timestamp_set
);
nf!(
    DT_SET_MICROSECOND,
    "public function setMicrosecond(int $microsecond): static {}",
    dt_set_microsecond
);
nf!(
    DT_GET_TIMESTAMP,
    "public function getTimestamp(): int {}",
    date_timestamp_get
);
nf!(
    DT_DIFF,
    "public function diff(DateTimeInterface $targetObject, bool $absolute = false): DateInterval {}",
    date_diff
);

// DateTimeImmutable
nf!(
    DTI_SET_STATE,
    "public static function __set_state(array $array): static {}",
    set_state
);
nf!(
    DTI_CREATE_FROM_FORMAT,
    "public static function createFromFormat(string $format, string $datetime, ?DateTimeZone $timezone = null): DateTimeImmutable|false {}",
    create_from_format_method
);
nf!(
    DTI_MODIFY,
    "public function modify(string $modifier): static {}",
    dti_modify
);
nf!(
    DTI_ADD,
    "public function add(DateInterval $interval): static {}",
    dti_add
);
nf!(
    DTI_SUB,
    "public function sub(DateInterval $interval): static {}",
    dti_sub
);
nf!(
    DTI_SET_TIMEZONE,
    "public function setTimezone(DateTimeZone $timezone): static {}",
    dti_set_timezone
);
nf!(
    DTI_SET_TIME,
    "public function setTime(int $hour, int $minute, int $second = 0, int $microsecond = 0): static {}",
    dti_set_time
);
nf!(
    DTI_SET_DATE,
    "public function setDate(int $year, int $month, int $day): static {}",
    dti_set_date
);
nf!(
    DTI_SET_ISODATE,
    "public function setISODate(int $year, int $week, int $dayOfWeek = 1): static {}",
    dti_set_isodate
);
nf!(
    DTI_SET_TIMESTAMP,
    "public function setTimestamp(int $timestamp): static {}",
    dti_set_timestamp
);
nf!(
    DTI_SET_MICROSECOND,
    "public function setMicrosecond(int $microsecond): static {}",
    dti_set_microsecond
);
nf!(
    DTI_CREATE_FROM_MUTABLE,
    "public static function createFromMutable(DateTime $object): static {}",
    create_from_object
);
nf!(
    DTI_CREATE_FROM_INTERFACE,
    "public static function createFromInterface(DateTimeInterface $object): static {}",
    create_from_object
);

/// The `DATE_*` format constants (`DateTimeInterface::*`).
pub const FORMATS: &[(&str, &str)] = &[
    ("ATOM", "Y-m-d\\TH:i:sP"),
    ("COOKIE", "l, d-M-Y H:i:s T"),
    ("ISO8601", "Y-m-d\\TH:i:sO"),
    ("ISO8601_EXPANDED", "X-m-d\\TH:i:sP"),
    ("RFC822", "D, d M y H:i:s O"),
    ("RFC850", "l, d-M-y H:i:s T"),
    ("RFC1036", "D, d M y H:i:s O"),
    ("RFC1123", "D, d M Y H:i:s O"),
    ("RFC7231", "D, d M Y H:i:s \\G\\M\\T"),
    ("RFC2822", "D, d M Y H:i:s O"),
    ("RFC3339", "Y-m-d\\TH:i:sP"),
    ("RFC3339_EXTENDED", "Y-m-d\\TH:i:s.vP"),
    ("RSS", "D, d M Y H:i:s O"),
    ("W3C", "Y-m-d\\TH:i:sP"),
];

/// The deprecation suffix of `DATE_RFC7231` / `DateTimeInterface::RFC7231`.
pub const RFC7231_DEPRECATION: &str =
    "since 8.5, as this format ignores the associated timezone and always uses GMT";

pub fn add_methods(ce: &mut ClassEntry, class: &str, methods: &[(&'static NativeFunction, u32)]) {
    for (nf, flags) in methods {
        ce.add_method(Function::native_method(nf, class, *flags));
    }
}

pub const PUB: u32 = member_flags::PUBLIC;
pub const STATIC: u32 = member_flags::PUBLIC | member_flags::STATIC;
const ABSTRACT: u32 = member_flags::PUBLIC | member_flags::ABSTRACT;
const NODISCARD: u32 = member_flags::PUBLIC | fn_flags::NODISCARD;

fn set_date_handlers(ce: &mut ClassEntry) {
    ce.handlers.create_object = Some(create_date_object);
    ce.handlers.clone_obj = Some(clone_date_object);
    ce.handlers.compare = Some(compare_dates);
    ce.handlers.get_properties_for = Some(date_properties_for);
}

pub fn register(r: &mut Registry) {
    // DateTimeInterface
    let mut iface = ClassEntry::new(DATE_INTERFACE, class_flags::INTERFACE);
    let string_ty = TypeDecl::parse("string").ok();
    for (name, value) in FORMATS {
        let mut c = ClassConstant::new(*name, Value::str(value), DATE_INTERFACE);
        c.ty = string_ty.clone();
        if *name == "RFC7231" {
            c.flags |= member_flags::DEPRECATED;
        }
        iface.add_constant(c);
    }
    add_methods(
        &mut iface,
        DATE_INTERFACE,
        &[
            (&I_FORMAT, ABSTRACT),
            (&I_GET_TIMEZONE, ABSTRACT),
            (&I_GET_OFFSET, ABSTRACT),
            (&I_GET_TIMESTAMP, ABSTRACT),
            (&I_GET_MICROSECOND, ABSTRACT),
            (&I_DIFF, ABSTRACT),
            (&I_WAKEUP, ABSTRACT),
            (&I_SERIALIZE, ABSTRACT),
            (&I_UNSERIALIZE, ABSTRACT),
        ],
    );
    let iface = r.class(iface);

    // DateTime
    let mut ce = ClassEntry::new(DATE_CLASS, 0);
    ce.interfaces.push(iface.clone());
    ce.constants = iface.constants.clone();
    add_methods(
        &mut ce,
        DATE_CLASS,
        &[
            (&DT_CONSTRUCT, PUB | fn_flags::CTOR),
            (&DT_SERIALIZE, PUB),
            (&DT_UNSERIALIZE, PUB),
            (&DT_WAKEUP, PUB),
            (&DT_SET_STATE, STATIC),
            (&DT_CREATE_FROM_IMMUTABLE, STATIC),
            (&DT_CREATE_FROM_INTERFACE, STATIC),
            (&DT_CREATE_FROM_FORMAT, STATIC),
            (&DT_CREATE_FROM_TIMESTAMP, STATIC),
            (&DT_GET_LAST_ERRORS, STATIC),
            (&DT_FORMAT, PUB),
            (&DT_MODIFY, PUB),
            (&DT_ADD, PUB),
            (&DT_SUB, PUB),
            (&DT_GET_TIMEZONE, PUB),
            (&DT_SET_TIMEZONE, PUB),
            (&DT_GET_OFFSET, PUB),
            (&DT_GET_MICROSECOND, PUB),
            (&DT_SET_TIME, PUB),
            (&DT_SET_DATE, PUB),
            (&DT_SET_ISODATE, PUB),
            (&DT_SET_TIMESTAMP, PUB),
            (&DT_SET_MICROSECOND, PUB),
            (&DT_GET_TIMESTAMP, PUB),
            (&DT_DIFF, PUB),
        ],
    );
    set_date_handlers(&mut ce);
    r.class(ce);

    // DateTimeImmutable
    let mut ce = ClassEntry::new(IMMUTABLE_CLASS, 0);
    ce.interfaces.push(iface.clone());
    ce.constants = iface.constants.clone();
    add_methods(
        &mut ce,
        IMMUTABLE_CLASS,
        &[
            (&DT_CONSTRUCT, PUB | fn_flags::CTOR),
            (&DT_SERIALIZE, PUB),
            (&DT_UNSERIALIZE, PUB),
            (&DT_WAKEUP, PUB),
            (&DTI_SET_STATE, STATIC),
            (&DTI_CREATE_FROM_FORMAT, STATIC),
            (&DT_CREATE_FROM_TIMESTAMP, STATIC),
            (&DT_GET_LAST_ERRORS, STATIC),
            (&DT_FORMAT, PUB),
            (&DT_GET_TIMEZONE, PUB),
            (&DT_GET_OFFSET, PUB),
            (&DT_GET_TIMESTAMP, PUB),
            (&DT_GET_MICROSECOND, PUB),
            (&DT_DIFF, PUB),
            (&DTI_MODIFY, NODISCARD),
            (&DTI_ADD, NODISCARD),
            (&DTI_SUB, NODISCARD),
            (&DTI_SET_TIMEZONE, NODISCARD),
            (&DTI_SET_TIME, NODISCARD),
            (&DTI_SET_DATE, NODISCARD),
            (&DTI_SET_ISODATE, NODISCARD),
            (&DTI_SET_TIMESTAMP, NODISCARD),
            (&DTI_SET_MICROSECOND, NODISCARD),
            (&DTI_CREATE_FROM_MUTABLE, STATIC),
            (&DTI_CREATE_FROM_INTERFACE, STATIC),
        ],
    );
    set_date_handlers(&mut ce);
    r.class(ce);
}
