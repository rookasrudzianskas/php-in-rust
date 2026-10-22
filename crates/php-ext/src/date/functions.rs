//! The procedural functions of php_date.c that do not operate on date
//! objects: `strtotime`, `date`, `gmdate`, `idate`, `mktime`, `gmmktime`,
//! `checkdate`, `strftime`, `gmstrftime`, `time`, `localtime`, `getdate`,
//! `date_parse`, `date_parse_from_format`, `date_get_last_errors`,
//! `timezone_name_from_abbr`, `timezone_identifiers_list`,
//! `timezone_abbreviations_list`, `timezone_version_get`,
//! `date_default_timezone_set/get`, `date_sunrise`, `date_sunset` and
//! `date_sun_info`.

use php_runtime::value::convert::to_float_silent;
use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Value, Vm};
use php_timelib::dow::{day_of_week, day_of_year, valid_date};
use php_timelib::format::{DAY_FULL_NAMES, MON_FULL_NAMES};
use php_timelib::{
    ErrorContainer, NO_CLONE, SPECIAL_FIRST_DAY_OF_MONTH, SPECIAL_WEEKDAY, SunFormat, SunResult,
    SunValue, Time, UNSET, ZoneType, builtin_db, date_format, idate as timelib_idate,
    parse_from_format, php_mktime, strtotime as timelib_strtotime, sun_info, sunrise_sunset,
    timezone_abbreviations_list as abbr_table, timezone_id_from_abbr, timezone_id_is_valid,
};

use super::common::{c_str, check_path, get_timezone_info, globals, php_time, tz_get};
use super::strftime::{Tm, strftime as c_strftime};

pub const SUNFUNCS_RET_TIMESTAMP: i64 = 0;
pub const SUNFUNCS_RET_STRING: i64 = 1;
pub const SUNFUNCS_RET_DOUBLE: i64 = 2;

pub const TIMEZONE_GROUP_AFRICA: i64 = 0x0001;
pub const TIMEZONE_GROUP_AMERICA: i64 = 0x0002;
pub const TIMEZONE_GROUP_ANTARCTICA: i64 = 0x0004;
pub const TIMEZONE_GROUP_ARCTIC: i64 = 0x0008;
pub const TIMEZONE_GROUP_ASIA: i64 = 0x0010;
pub const TIMEZONE_GROUP_ATLANTIC: i64 = 0x0020;
pub const TIMEZONE_GROUP_AUSTRALIA: i64 = 0x0040;
pub const TIMEZONE_GROUP_EUROPE: i64 = 0x0080;
pub const TIMEZONE_GROUP_INDIAN: i64 = 0x0100;
pub const TIMEZONE_GROUP_PACIFIC: i64 = 0x0200;
pub const TIMEZONE_GROUP_UTC: i64 = 0x0400;
pub const TIMEZONE_GROUP_ALL: i64 = 0x07FF;
pub const TIMEZONE_GROUP_ALL_W_BC: i64 = 0x0FFF;
pub const TIMEZONE_PER_COUNTRY: i64 = 0x1000;

/// A local time for `ts` in the default timezone.
fn local_time(vm: &mut Vm, ts: i64) -> PResult<Time> {
    let tzi = get_timezone_info(vm)?;
    let mut t = Time {
        tz_info: Some(tzi),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    t.unixtime2local(ts);
    Ok(t)
}

/// `php_format_date`: `date()` (`localtime`) or `gmdate()`.
pub fn php_format_date(vm: &mut Vm, format: &[u8], ts: i64, localtime: bool) -> PResult<Vec<u8>> {
    if localtime {
        let t = local_time(vm, ts)?;
        Ok(date_format(format, &t, true))
    } else {
        let mut t = Time::new();
        t.unixtime2gmt(ts);
        Ok(date_format(format, &t, false))
    }
}

fn strtotime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let times = args.str(0).clone();
    // timelib_strtotime() expects the string to not be empty
    if times.is_empty() {
        return Ok(Value::Bool(false));
    }
    let tzi = get_timezone_info(vm)?;
    let mut now = Time {
        tz_info: Some(tzi.clone()),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    now.unixtime2local(args.int_opt(1).unwrap_or_else(php_time));
    let res = timelib_strtotime(times.as_bytes(), &tz_get);
    if res.errors.error_count() > 0 {
        return Ok(Value::Bool(false));
    }
    let mut t = res.time;
    t.fill_holes(&now, NO_CLONE);
    t.update_ts(Some(&tzi));
    Ok(Value::Int(t.sse))
}

fn date(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ts = args.int_opt(1).unwrap_or_else(php_time);
    let s = php_format_date(vm, args.str(0).as_bytes(), ts, true)?;
    Ok(Value::from(s))
}

fn gmdate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ts = args.int_opt(1).unwrap_or_else(php_time);
    let s = php_format_date(vm, args.str(0).as_bytes(), ts, false)?;
    Ok(Value::from(s))
}

fn idate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let format = args.str(0).clone();
    if format.len() != 1 {
        vm.warning("idate format is one char")?;
        return Ok(Value::Bool(false));
    }
    let ts = args.int_opt(1).unwrap_or_else(php_time);
    let t = local_time(vm, ts)?;
    match timelib_idate(format.as_bytes()[0], &t) {
        Some(v) => Ok(Value::Int(i64::from(v))),
        None => {
            vm.warning("Unrecognized date format token")?;
            Ok(Value::Bool(false))
        }
    }
}

fn mktime_common(vm: &mut Vm, args: &Args, gmt: bool) -> PResult<Value> {
    let tzi = if gmt {
        None
    } else {
        Some(get_timezone_info(vm)?)
    };
    let ts = php_mktime(
        args.int(0),
        args.int_opt(1),
        args.int_opt(2),
        args.int_opt(3),
        args.int_opt(4),
        args.int_opt(5),
        php_time(),
        tzi.as_ref(),
    );
    match ts {
        Some(ts) => Ok(Value::Int(ts)),
        None => {
            vm.warning("Epoch doesn't fit in a PHP integer")?;
            Ok(Value::Bool(false))
        }
    }
}

fn mktime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    mktime_common(vm, args, false)
}

fn gmmktime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    mktime_common(vm, args, true)
}

fn checkdate(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (m, d, y) = (args.int(0), args.int(1), args.int(2));
    Ok(Value::Bool((1..=32767).contains(&y) && valid_date(y, m, d)))
}

/// `php_strftime`.
fn strftime_common(vm: &mut Vm, args: &Args, gmt: bool) -> PResult<Value> {
    let name = if gmt { "gmstrftime" } else { "strftime" };
    vm.engine_deprecated(format!(
        "Function {name}() is deprecated since 8.1, use IntlDateFormatter::format() instead"
    ))?;
    let format = args.str(0).clone();
    if format.is_empty() {
        return Ok(Value::Bool(false));
    }
    let timestamp = args.int_opt(1).unwrap_or_else(php_time);
    let mut t = Time::new();
    let tzi = if gmt {
        t.unixtime2gmt(timestamp);
        None
    } else {
        let tzi = get_timezone_info(vm)?;
        t.tz_info = Some(tzi.clone());
        t.zone_type = ZoneType::Id;
        t.unixtime2local(timestamp);
        Some(tzi)
    };
    let mut tm = Tm {
        sec: t.s,
        min: t.i,
        hour: t.h,
        mday: t.d,
        mon: t.m - 1,
        year: t.y - 1900,
        wday: day_of_week(t.y, t.m, t.d),
        yday: day_of_year(t.y, t.m, t.d),
        ts: timestamp,
        ..Tm::default()
    };
    match &tzi {
        None => {
            tm.isdst = 0;
            tm.gmtoff = 0;
            tm.zone = "GMT".to_string();
        }
        Some(tzi) => {
            let offset = tzi.get_time_zone_info(timestamp);
            tm.isdst = i64::from(offset.is_dst);
            tm.gmtoff = i64::from(offset.offset);
            tm.zone = offset.abbr;
        }
    }
    // strftime(3) works on C strings.
    let out = c_strftime(c_str(format.as_bytes()), &tm);
    // php_strftime gives up after growing its buffer to 4096 bytes, and
    // treats an empty result as a failure.
    if out.is_empty() || out.len() >= 4096 {
        return Ok(Value::Bool(false));
    }
    Ok(Value::from(out))
}

fn strftime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strftime_common(vm, args, false)
}

fn gmstrftime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strftime_common(vm, args, true)
}

fn time(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(php_time()))
}

fn localtime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let timestamp = args.int_opt(0).unwrap_or_else(php_time);
    let associative = args.bool(1);
    let ts = local_time(vm, timestamp)?;
    let fields = [
        ("tm_sec", ts.s),
        ("tm_min", ts.i),
        ("tm_hour", ts.h),
        ("tm_mday", ts.d),
        ("tm_mon", ts.m - 1),
        ("tm_year", ts.y - 1900),
        ("tm_wday", day_of_week(ts.y, ts.m, ts.d)),
        ("tm_yday", day_of_year(ts.y, ts.m, ts.d)),
        ("tm_isdst", i64::from(ts.dst)),
    ];
    let mut a = PhpArray::with_capacity(9);
    for (k, v) in fields {
        if associative {
            a.insert_str(k, v);
        } else {
            let _ = a.append(v);
        }
    }
    Ok(Value::array(a))
}

fn getdate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let timestamp = args.int_opt(0).unwrap_or_else(php_time);
    let ts = local_time(vm, timestamp)?;
    let mut a = PhpArray::with_capacity(11);
    a.insert_str("seconds", ts.s);
    a.insert_str("minutes", ts.i);
    a.insert_str("hours", ts.h);
    a.insert_str("mday", ts.d);
    a.insert_str("wday", day_of_week(ts.y, ts.m, ts.d));
    a.insert_str("mon", ts.m);
    a.insert_str("year", ts.y);
    a.insert_str("yday", day_of_year(ts.y, ts.m, ts.d));
    let dow = usize::try_from(day_of_week(ts.y, ts.m, ts.d)).ok();
    a.insert_str(
        "weekday",
        dow.and_then(|i| DAY_FULL_NAMES.get(i))
            .copied()
            .unwrap_or("Unknown"),
    );
    a.insert_str(
        "month",
        usize::try_from(ts.m - 1)
            .ok()
            .and_then(|i| MON_FULL_NAMES.get(i))
            .copied()
            .unwrap_or(""),
    );
    a.insert(ArrayKey::Int(0), Value::Int(timestamp));
    Ok(Value::array(a))
}

/// `zval_from_error_container`.
pub fn errors_to_array(a: &mut PhpArray, error: &ErrorContainer) {
    let list = |msgs: &[php_timelib::ErrorMessage]| {
        let mut e = PhpArray::with_capacity(msgs.len());
        for m in msgs {
            e.insert(ArrayKey::Int(m.position as i64), Value::str(m.message));
        }
        Value::array(e)
    };
    a.insert_str("warning_count", error.warning_count() as i64);
    a.insert_str("warnings", list(&error.warnings));
    a.insert_str("error_count", error.error_count() as i64);
    a.insert_str("errors", list(&error.errors));
}

/// `date_get_last_errors()`.
pub fn last_errors_value(vm: &mut Vm) -> Value {
    match &globals(vm).last_errors {
        Some(err) => {
            let mut a = PhpArray::new();
            errors_to_array(&mut a, err);
            Value::array(a)
        }
        None => Value::Bool(false),
    }
}

fn date_get_last_errors(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(last_errors_value(vm))
}

/// `php_date_do_return_parsed_time`.
fn parsed_time_to_array(t: &Time, error: &ErrorContainer) -> Value {
    let mut a = PhpArray::new();
    let set = |a: &mut PhpArray, name: &str, v: i64| {
        if v == UNSET {
            a.insert_str(name, false);
        } else {
            a.insert_str(name, v);
        }
    };
    set(&mut a, "year", t.y);
    set(&mut a, "month", t.m);
    set(&mut a, "day", t.d);
    set(&mut a, "hour", t.h);
    set(&mut a, "minute", t.i);
    set(&mut a, "second", t.s);
    if t.us == UNSET {
        a.insert_str("fraction", false);
    } else {
        a.insert_str("fraction", t.us as f64 / 1_000_000.0);
    }
    errors_to_array(&mut a, error);
    a.insert_str("is_localtime", t.is_localtime);
    if t.is_localtime {
        set(&mut a, "zone_type", i64::from(t.zone_type.as_i32()));
        match t.zone_type {
            ZoneType::Offset => {
                set(&mut a, "zone", i64::from(t.z));
                a.insert_str("is_dst", t.dst != 0);
            }
            ZoneType::Id => {
                if let Some(abbr) = &t.tz_abbr {
                    a.insert_str("tz_abbr", abbr.as_str());
                }
                if let Some(tz) = &t.tz_info {
                    a.insert_str("tz_id", tz.name.as_str());
                }
            }
            ZoneType::Abbr => {
                set(&mut a, "zone", i64::from(t.z));
                a.insert_str("is_dst", t.dst != 0);
                a.insert_str("tz_abbr", t.tz_abbr.as_deref().unwrap_or(""));
            }
            ZoneType::None => {}
        }
    }
    if t.have_relative {
        let r = &t.relative;
        let mut e = PhpArray::new();
        e.insert_str("year", r.y);
        e.insert_str("month", r.m);
        e.insert_str("day", r.d);
        e.insert_str("hour", r.h);
        e.insert_str("minute", r.i);
        e.insert_str("second", r.s);
        if r.have_weekday_relative {
            e.insert_str("weekday", i64::from(r.weekday));
        }
        if r.have_special_relative && r.special.kind == SPECIAL_WEEKDAY {
            e.insert_str("weekdays", r.special.amount);
        }
        if r.first_last_day_of != 0 {
            let key = if r.first_last_day_of == SPECIAL_FIRST_DAY_OF_MONTH {
                "first_day_of_month"
            } else {
                "last_day_of_month"
            };
            e.insert_str(key, true);
        }
        a.insert_str("relative", Value::array(e));
    }
    Value::array(a)
}

fn date_parse(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = timelib_strtotime(args.str(0).as_bytes(), &tz_get);
    Ok(parsed_time_to_array(&res.time, &res.errors))
}

fn date_parse_from_format(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    check_path(vm, 2, args.str(1).as_bytes())?;
    let res = parse_from_format(args.str(0).as_bytes(), args.str(1).as_bytes(), &tz_get);
    Ok(parsed_time_to_array(&res.time, &res.errors))
}

pub fn timezone_name_from_abbr(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let abbr = args.str(0).clone();
    let abbr = String::from_utf8_lossy(c_str(abbr.as_bytes())).into_owned();
    let isdst = args.int(2) as i32;
    match timezone_id_from_abbr(&abbr, args.int(1), isdst) {
        Some(id) => Ok(Value::str(id)),
        None => Ok(Value::Bool(false)),
    }
}

/// `check_id_allowed`.
fn check_id_allowed(id: &str, what: i64) -> bool {
    const GROUPS: [(i64, &str); 11] = [
        (TIMEZONE_GROUP_AFRICA, "Africa/"),
        (TIMEZONE_GROUP_AMERICA, "America/"),
        (TIMEZONE_GROUP_ANTARCTICA, "Antarctica/"),
        (TIMEZONE_GROUP_ARCTIC, "Arctic/"),
        (TIMEZONE_GROUP_ASIA, "Asia/"),
        (TIMEZONE_GROUP_ATLANTIC, "Atlantic/"),
        (TIMEZONE_GROUP_AUSTRALIA, "Australia/"),
        (TIMEZONE_GROUP_EUROPE, "Europe/"),
        (TIMEZONE_GROUP_INDIAN, "Indian/"),
        (TIMEZONE_GROUP_PACIFIC, "Pacific/"),
        (TIMEZONE_GROUP_UTC, "UTC"),
    ];
    GROUPS.iter().any(|(bit, prefix)| {
        what & bit != 0
            && id.len() >= prefix.len()
            && id.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    })
}

pub fn timezone_identifiers_list(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let what = args.int(0);
    let option = args.str_opt(1).cloned().unwrap_or_default();
    if what == TIMEZONE_PER_COUNTRY && option.len() != 2 {
        return Err(vm.argument_value_error(
            2,
            "must be a two-letter ISO 3166-1 compatible country code when argument #1 ($timezoneGroup) is DateTimeZone::PER_COUNTRY",
        ));
    }
    let db = builtin_db();
    let mut a = PhpArray::new();
    for entry in db.identifiers() {
        let take = if what == TIMEZONE_PER_COUNTRY {
            db.entry_country_code(entry) == [option.as_bytes()[0], option.as_bytes()[1]]
        } else {
            what == TIMEZONE_GROUP_ALL_W_BC
                || (check_id_allowed(&entry.id, what) && db.entry_is_bc(entry))
        };
        if take {
            let _ = a.append(entry.id.as_str());
        }
    }
    Ok(Value::array(a))
}

pub fn timezone_abbreviations_list(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let mut out = PhpArray::new();
    for entry in abbr_table() {
        let mut e = PhpArray::with_capacity(3);
        e.insert_str("dst", entry.dst);
        e.insert_str("offset", i64::from(entry.gmtoffset));
        match entry.full_tz_name {
            Some(n) => e.insert_str("timezone_id", n),
            None => e.insert_str("timezone_id", Value::Null),
        };
        let key = ArrayKey::from_bytes(entry.name.as_bytes());
        let slot = out.entry_mut(key);
        if !matches!(slot, Value::Array(_)) {
            *slot = Value::array(PhpArray::new());
        }
        if let Value::Array(list) = slot {
            let _ = std::rc::Rc::make_mut(list).append(Value::array(e));
        }
    }
    Ok(Value::array(out))
}

fn timezone_version_get(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::str(builtin_db().version()))
}

fn date_default_timezone_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let zone = args.str(0).clone();
    let name = String::from_utf8_lossy(c_str(zone.as_bytes())).into_owned();
    if !timezone_id_is_valid(&name) {
        let mut msg = b"Timezone ID '".to_vec();
        msg.extend_from_slice(c_str(zone.as_bytes()));
        msg.extend_from_slice(b"' is invalid");
        vm.notice(PhpStr::from_vec(msg))?;
        return Ok(Value::Bool(false));
    }
    globals(vm).timezone = Some(name);
    Ok(Value::Bool(true))
}

fn date_default_timezone_get(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let tz = get_timezone_info(vm)?;
    Ok(Value::str(&tz.name))
}

/// `zend_ini_double_literal`.
fn ini_double(vm: &Vm, name: &str) -> f64 {
    vm.ini_get(name)
        .map_or(0.0, |v| to_float_silent(&Value::String(v)))
}

/// `php_do_date_sunrise_sunset`.
fn sunrise_sunset_common(vm: &mut Vm, args: &Args, calc_sunset: bool) -> PResult<Value> {
    let name = if calc_sunset {
        "date_sunset"
    } else {
        "date_sunrise"
    };
    vm.engine_deprecated(format!(
        "Function {name}() is deprecated since 8.1, use date_sun_info() instead"
    ))?;
    let time = args.int(0);
    let retformat = args.int(1);
    let latitude = args
        .float_opt(2)
        .unwrap_or_else(|| ini_double(vm, "date.default_latitude"));
    let longitude = args
        .float_opt(3)
        .unwrap_or_else(|| ini_double(vm, "date.default_longitude"));
    let zenith = args.float_opt(4).unwrap_or_else(|| {
        ini_double(
            vm,
            if calc_sunset {
                "date.sunset_zenith"
            } else {
                "date.sunrise_zenith"
            },
        )
    });
    let format = match retformat {
        SUNFUNCS_RET_TIMESTAMP => SunFormat::Timestamp,
        SUNFUNCS_RET_STRING => SunFormat::String,
        SUNFUNCS_RET_DOUBLE => SunFormat::Double,
        _ => {
            return Err(vm.argument_value_error(
                2,
                "must be one of SUNFUNCS_RET_TIMESTAMP, SUNFUNCS_RET_STRING, or SUNFUNCS_RET_DOUBLE",
            ));
        }
    };
    if !latitude.is_finite() || !longitude.is_finite() {
        return Ok(Value::Bool(false));
    }
    let tzi = get_timezone_info(vm)?;
    let r = sunrise_sunset(
        time,
        format,
        latitude,
        longitude,
        zenith,
        args.float_opt(5),
        &tzi,
        calc_sunset,
    );
    Ok(match r {
        None => Value::Bool(false),
        Some(SunResult::Timestamp(ts)) => Value::Int(ts),
        Some(SunResult::String(s)) => Value::from(s),
        Some(SunResult::Double(d)) => Value::Float(d),
    })
}

fn date_sunrise(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    sunrise_sunset_common(vm, args, false)
}

fn date_sunset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    sunrise_sunset_common(vm, args, true)
}

fn date_sun_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let time = args.int(0);
    let latitude = args.float(1);
    let longitude = args.float(2);
    if !latitude.is_finite() {
        return Err(vm.argument_value_error(2, "must be finite"));
    }
    if !longitude.is_finite() {
        return Err(vm.argument_value_error(3, "must be finite"));
    }
    let tzi = get_timezone_info(vm)?;
    let si = sun_info(time, latitude, longitude, &tzi);
    let v = |s: SunValue| match s {
        SunValue::Time(t) => Value::Int(t),
        SunValue::Always(b) => Value::Bool(b),
    };
    let mut a = PhpArray::with_capacity(9);
    a.insert_str("sunrise", v(si.sunrise));
    a.insert_str("sunset", v(si.sunset));
    a.insert_str("transit", si.transit);
    a.insert_str("civil_twilight_begin", v(si.civil_twilight_begin));
    a.insert_str("civil_twilight_end", v(si.civil_twilight_end));
    a.insert_str("nautical_twilight_begin", v(si.nautical_twilight_begin));
    a.insert_str("nautical_twilight_end", v(si.nautical_twilight_end));
    a.insert_str(
        "astronomical_twilight_begin",
        v(si.astronomical_twilight_begin),
    );
    a.insert_str("astronomical_twilight_end", v(si.astronomical_twilight_end));
    Ok(Value::array(a))
}

pub static STRTOTIME: NativeFunction = NativeFunction {
    sig: "function strtotime(string $datetime, ?int $baseTimestamp = null): int|false {}",
    handler: strtotime,
};
pub static DATE: NativeFunction = NativeFunction {
    sig: "function date(string $format, ?int $timestamp = null): string {}",
    handler: date,
};
pub static IDATE: NativeFunction = NativeFunction {
    sig: "function idate(string $format, ?int $timestamp = null): int|false {}",
    handler: idate,
};
pub static GMDATE: NativeFunction = NativeFunction {
    sig: "function gmdate(string $format, ?int $timestamp = null): string {}",
    handler: gmdate,
};
pub static MKTIME: NativeFunction = NativeFunction {
    sig: "function mktime(
    int $hour, ?int $minute = null, ?int $second = null,
    ?int $month = null, ?int $day = null, ?int $year = null): int|false {}",
    handler: mktime,
};
pub static GMMKTIME: NativeFunction = NativeFunction {
    sig: "function gmmktime(
    int $hour, ?int $minute = null, ?int $second = null,
    ?int $month = null, ?int $day = null, ?int $year = null): int|false {}",
    handler: gmmktime,
};
pub static CHECKDATE: NativeFunction = NativeFunction {
    sig: "function checkdate(int $month, int $day, int $year): bool {}",
    handler: checkdate,
};
// The stub's `#[\Deprecated]` attribute is emitted by the handlers.
pub static STRFTIME: NativeFunction = NativeFunction {
    sig: "function strftime(string $format, ?int $timestamp = null): string|false {}",
    handler: strftime,
};
pub static GMSTRFTIME: NativeFunction = NativeFunction {
    sig: "function gmstrftime(string $format, ?int $timestamp = null): string|false {}",
    handler: gmstrftime,
};
pub static TIME: NativeFunction = NativeFunction {
    sig: "function time(): int {}",
    handler: time,
};
pub static LOCALTIME: NativeFunction = NativeFunction {
    sig: "function localtime(?int $timestamp = null, bool $associative = false): array {}",
    handler: localtime,
};
pub static GETDATE: NativeFunction = NativeFunction {
    sig: "function getdate(?int $timestamp = null): array {}",
    handler: getdate,
};
pub static DATE_PARSE: NativeFunction = NativeFunction {
    sig: "function date_parse(string $datetime): array {}",
    handler: date_parse,
};
pub static DATE_PARSE_FROM_FORMAT: NativeFunction = NativeFunction {
    sig: "function date_parse_from_format(string $format, string $datetime): array {}",
    handler: date_parse_from_format,
};
pub static DATE_GET_LAST_ERRORS: NativeFunction = NativeFunction {
    sig: "function date_get_last_errors(): array|false {}",
    handler: date_get_last_errors,
};
pub static TIMEZONE_NAME_FROM_ABBR: NativeFunction = NativeFunction {
    sig: "function timezone_name_from_abbr(string $abbr, int $utcOffset = -1, int $isDST = -1): string|false {}",
    handler: timezone_name_from_abbr,
};
pub static TIMEZONE_IDENTIFIERS_LIST: NativeFunction = NativeFunction {
    sig: "function timezone_identifiers_list(int $timezoneGroup = DateTimeZone::ALL, ?string $countryCode = null): array {}",
    handler: timezone_identifiers_list,
};
pub static TIMEZONE_ABBREVIATIONS_LIST: NativeFunction = NativeFunction {
    sig: "function timezone_abbreviations_list(): array {}",
    handler: timezone_abbreviations_list,
};
pub static TIMEZONE_VERSION_GET: NativeFunction = NativeFunction {
    sig: "function timezone_version_get(): string {}",
    handler: timezone_version_get,
};
pub static DATE_DEFAULT_TIMEZONE_SET: NativeFunction = NativeFunction {
    sig: "function date_default_timezone_set(string $timezoneId): bool {}",
    handler: date_default_timezone_set,
};
pub static DATE_DEFAULT_TIMEZONE_GET: NativeFunction = NativeFunction {
    sig: "function date_default_timezone_get(): string {}",
    handler: date_default_timezone_get,
};
pub static DATE_SUNRISE: NativeFunction = NativeFunction {
    sig: "function date_sunrise(
    int $timestamp, int $returnFormat = SUNFUNCS_RET_STRING,
    ?float $latitude = null, ?float $longitude = null, ?float $zenith = null,
    ?float $utcOffset = null): string|int|float|false {}",
    handler: date_sunrise,
};
pub static DATE_SUNSET: NativeFunction = NativeFunction {
    sig: "function date_sunset(
    int $timestamp, int $returnFormat = SUNFUNCS_RET_STRING,
    ?float $latitude = null, ?float $longitude = null, ?float $zenith = null,
    ?float $utcOffset = null): string|int|float|false {}",
    handler: date_sunset,
};
pub static DATE_SUN_INFO: NativeFunction = NativeFunction {
    sig: "function date_sun_info(int $timestamp, float $latitude, float $longitude): array {}",
    handler: date_sun_info,
};
