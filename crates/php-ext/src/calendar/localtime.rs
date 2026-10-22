//! The C library's notion of local time, as used by php-src's calendar code.
//!
//! `easter_date()` calls `mktime()`, and `unixtojd()` and `easter_*()` call
//! `localtime_r()`. Neither goes through ext/date: libc picks the zone from the
//! `TZ` environment variable (`putenv('TZ=...')` takes effect, while
//! `date_default_timezone_set()` does not) and falls back to the system zone.
//! This module reproduces that on top of the bundled timezone database.

use std::ffi::OsStr;
use std::sync::Arc;

use php_timelib::{Time, TzInfo, ZoneType, builtin_tzinfo};

fn utc() -> Arc<TzInfo> {
    builtin_tzinfo("UTC").expect("the bundled timezone database contains UTC")
}

/// A zone name from `TZ`: an identifier, optionally prefixed by `:` or given
/// as a path into a zoneinfo directory. Anything unknown means UTC, like libc.
pub(super) fn zone_from_name(name: &str) -> Arc<TzInfo> {
    let name = name.strip_prefix(':').unwrap_or(name);
    let name = name
        .split_once("zoneinfo/")
        .map_or(name, |(_, identifier)| identifier);
    builtin_tzinfo(name).unwrap_or_else(utc)
}

/// The zone libc uses for `localtime_r()` and `mktime()`: `TZ` when set,
/// otherwise the system zone (`/etc/localtime`), otherwise UTC.
pub fn libc_timezone() -> Arc<TzInfo> {
    match std::env::var_os("TZ") {
        Some(tz) => zone_from_name(&tz.to_string_lossy()),
        None => system_timezone(),
    }
}

/// The zone `/etc/localtime` points at.
fn system_timezone() -> Arc<TzInfo> {
    let Ok(target) = std::fs::read_link(OsStr::new("/etc/localtime")) else {
        return utc();
    };
    let target = target.to_string_lossy();
    match target.split_once("zoneinfo/") {
        Some((_, identifier)) => builtin_tzinfo(identifier).unwrap_or_else(utc),
        None => utc(),
    }
}

/// A date as `struct tm` holds it (`tm_year` is relative to 1900, `tm_mon`
/// starts at 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalDate {
    pub tm_year: i32,
    pub tm_mon: i32,
    pub tm_mday: i32,
}

/// `localtime_r()`: the local date of a timestamp, `None` when `tm_year` does
/// not fit an `int` (libc returns NULL with `EOVERFLOW`).
pub fn localtime(tz: &Arc<TzInfo>, timestamp: i64) -> Option<LocalDate> {
    let mut t = Time::new();
    t.tz_info = Some(tz.clone());
    t.zone_type = ZoneType::Id;
    t.unixtime2local(timestamp);
    Some(LocalDate {
        tm_year: i32::try_from(t.y.checked_sub(1900)?).ok()?,
        tm_mon: (t.m - 1) as i32,
        tm_mday: t.d as i32,
    })
}

/// `mktime()` of midnight on the given date with `tm_isdst = -1`.
pub fn mktime_midnight(tz: &Arc<TzInfo>, year: i64, month: i64, day: i64) -> i64 {
    let mut t = Time::new();
    t.tz_info = Some(tz.clone());
    t.zone_type = ZoneType::Id;
    t.y = year;
    t.m = month;
    t.d = day;
    t.update_ts(Some(tz));
    t.sse
}

/// `time(NULL)`.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
