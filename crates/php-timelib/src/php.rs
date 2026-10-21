//! Thin PHP-level glue over timelib, ported from `ext/date/php_date.c`: the exact
//! sequences of timelib calls that `strtotime()`, `mktime()`, `new DateTime()`,
//! `DateTime::modify()`, `setTimezone()`, `add()`, `sub()`, `diff()`,
//! `new DateTimeZone()` and `new DateInterval()` perform. They take the default
//! timezone and the current time explicitly, so they are deterministic.

use std::sync::Arc;

use crate::dow::daynr_from_weeknr;
use crate::error::ErrorContainer;
use crate::interval::{add, add_wall, diff, sub, sub_wall};
use crate::parse::common::{Cur, parse_zone};
use crate::parse::from_format::parse_from_format;
use crate::parse::iso_interval::strtointerval;
use crate::parse::strtotime::strtotime;
use crate::time::{AbbrInfo, NO_CLONE, OVERRIDE_TIME, RelTime, Time, UNSET, ZoneType};
use crate::tz::{TzInfo, builtin_tzinfo};

/// PHP's `strtotime($input, $now)` with `date.timezone = tz`: returns `None` where
/// PHP returns `false` (parse errors or an empty string).
pub fn php_strtotime(input: impl AsRef<[u8]>, now: i64, tz: &Arc<TzInfo>) -> Option<i64> {
    let input = input.as_ref();
    if input.is_empty() {
        return None;
    }
    let mut now_t = Time {
        tz_info: Some(tz.clone()),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    now_t.unixtime2local(now);
    let res = strtotime(input, &builtin_tzinfo);
    if !res.errors.errors.is_empty() {
        return None;
    }
    let mut t = res.time;
    t.fill_holes(&now_t, NO_CLONE);
    t.update_ts(Some(tz));
    t.date_to_int()
}

/// PHP's `mktime()` / `gmmktime()` (`php_mktime`): `None` arguments take the value
/// from `now`; `tz = None` means `gmmktime()`.
#[allow(clippy::too_many_arguments)]
pub fn php_mktime(
    hour: i64,
    minute: Option<i64>,
    second: Option<i64>,
    month: Option<i64>,
    day: Option<i64>,
    year: Option<i64>,
    now: i64,
    tz: Option<&Arc<TzInfo>>,
) -> Option<i64> {
    let mut t = Time::new();
    match tz {
        None => t.unixtime2gmt(now),
        Some(tz) => {
            t.tz_info = Some(tz.clone());
            t.zone_type = ZoneType::Id;
            t.unixtime2local(now);
        }
    }
    t.h = hour;
    if let Some(v) = minute {
        t.i = v;
    }
    if let Some(v) = second {
        t.s = v;
    }
    if let Some(v) = month {
        t.m = v;
    }
    if let Some(v) = day {
        t.d = v;
    }
    if let Some(mut y) = year {
        if (0..70).contains(&y) {
            y += 2000;
        } else if (70..=100).contains(&y) {
            y += 1900;
        }
        t.y = y;
    }
    t.update_ts(tz);
    t.date_to_int()
}

/// The state of a `DateTimeZone` object (`php_timezone_obj`).
#[derive(Clone, Debug, PartialEq)]
pub enum PhpTimeZone {
    /// An identifier such as `Europe/Amsterdam`.
    Id(Arc<TzInfo>),
    /// A fixed offset in seconds such as `+02:00`.
    Offset(i64),
    /// An abbreviation such as `EST`.
    Abbr(AbbrInfo),
}

impl PhpTimeZone {
    /// `DateTimeZone::getName()`.
    pub fn name(&self) -> String {
        match self {
            PhpTimeZone::Id(tz) => tz.name.clone(),
            PhpTimeZone::Offset(o) => crate::format::tz_offset_str(*o),
            PhpTimeZone::Abbr(a) => a.abbr.clone(),
        }
    }

    /// The zone type.
    pub fn zone_type(&self) -> ZoneType {
        match self {
            PhpTimeZone::Id(_) => ZoneType::Id,
            PhpTimeZone::Offset(_) => ZoneType::Offset,
            PhpTimeZone::Abbr(_) => ZoneType::Abbr,
        }
    }

    /// `DateTime::getTimezone()` (`set_timezone_from_timelib_time`); `None` if the time
    /// has no zone.
    pub fn from_time(t: &Time) -> Option<PhpTimeZone> {
        match t.zone_type {
            ZoneType::Id => t.tz_info.clone().map(PhpTimeZone::Id),
            ZoneType::Offset => Some(PhpTimeZone::Offset(i64::from(t.z))),
            ZoneType::Abbr => Some(PhpTimeZone::Abbr(AbbrInfo {
                utc_offset: i64::from(t.z),
                abbr: t.tz_abbr.clone().unwrap_or_default(),
                dst: t.dst,
            })),
            ZoneType::None => None,
        }
    }
}

/// `new DateTimeZone($tz)` (`timezone_initialize`). The error is PHP's warning text,
/// e.g. `Unknown or bad timezone (Mars/Base)`.
pub fn timezone_open(tz: &str) -> Result<PhpTimeZone, String> {
    let mut dummy = Time::new();
    let mut c = Cur::new(tz.as_bytes());
    let (z, not_found) = parse_zone(&mut c, &mut dummy, &builtin_tzinfo);
    dummy.z = z as i32;
    if z >= 100 * 3600 || z <= -100 * 3600 {
        return Err(format!("Timezone offset is out of range ({tz})"));
    }
    if not_found || c.c() != 0 {
        return Err(format!("Unknown or bad timezone ({tz})"));
    }
    PhpTimeZone::from_time(&dummy).ok_or_else(|| format!("Unknown or bad timezone ({tz})"))
}

fn now_in_zone(zone: &PhpTimeZone, now_sec: i64, now_usec: i64) -> Time {
    let mut now = Time {
        zone_type: zone.zone_type(),
        ..Time::new()
    };
    match zone {
        PhpTimeZone::Id(tz) => now.tz_info = Some(tz.clone()),
        PhpTimeZone::Offset(o) => now.z = *o as i32,
        PhpTimeZone::Abbr(a) => {
            now.z = a.utc_offset as i32;
            now.dst = a.dst;
            now.tz_abbr = Some(a.abbr.clone());
        }
    }
    now.unixtime2local(now_sec);
    now.us = now_usec;
    now
}

/// `new DateTime($time, $timezone)` / `DateTime::createFromFormat($format, $time,
/// $timezone)` (`php_date_initialize`). `default_tz` is `date.timezone`; `now` is the
/// current time as `(seconds, microseconds)`. On failure returns the parse errors.
pub fn date_initialize(
    time_str: &[u8],
    format: Option<&[u8]>,
    timezone: Option<&PhpTimeZone>,
    default_tz: &Arc<TzInfo>,
    now: (i64, i64),
) -> Result<(Time, ErrorContainer), ErrorContainer> {
    let res = match format {
        Some(f) => parse_from_format(f, time_str, &builtin_tzinfo),
        None => {
            let s: &[u8] = if time_str.is_empty() {
                b"now"
            } else {
                time_str
            };
            strtotime(s, &builtin_tzinfo)
        }
    };
    if !res.errors.errors.is_empty() {
        return Err(res.errors);
    }
    let mut time = res.time;
    let zone = match timezone {
        Some(z) => z.clone(),
        None => match &time.tz_info {
            Some(tz) => PhpTimeZone::Id(tz.clone()),
            None => PhpTimeZone::Id(default_tz.clone()),
        },
    };
    let now_t = now_in_zone(&zone, now.0, now.1);
    if format.is_none() && (time_str == b"now" || time_str.is_empty()) {
        return Ok((now_t, res.errors));
    }
    let mut options = NO_CLONE;
    if format.is_some() {
        options |= OVERRIDE_TIME;
    }
    time.fill_holes(&now_t, options);
    let tzi = match &zone {
        PhpTimeZone::Id(tz) => Some(tz.clone()),
        _ => None,
    };
    time.update_ts(tzi.as_ref());
    time.update_from_sse();
    time.have_relative = false;
    Ok((time, res.errors))
}

/// `DateTime::modify($modify)` (`php_date_modify`). On failure the time is unchanged
/// and the parse errors are returned.
pub fn date_modify(t: &mut Time, modify: &[u8]) -> Result<ErrorContainer, ErrorContainer> {
    let res = strtotime(modify, &builtin_tzinfo);
    if !res.errors.errors.is_empty() {
        return Err(res.errors);
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
    Ok(res.errors)
}

/// `DateTime::setTimezone()` (`php_date_timezone_set`).
pub fn date_timezone_set(t: &mut Time, zone: &PhpTimeZone) {
    match zone {
        PhpTimeZone::Offset(o) => t.set_timezone_from_offset(*o),
        PhpTimeZone::Abbr(a) => t.set_timezone_from_abbr(a),
        PhpTimeZone::Id(tz) => t.set_timezone(tz),
    }
    t.unixtime2local(t.sse);
}

/// `DateTime::getOffset()` (`date_offset_get`).
pub fn date_offset_get(t: &Time) -> i64 {
    if !t.is_localtime {
        return 0;
    }
    match t.zone_type {
        ZoneType::Id => t
            .tz_info
            .as_deref()
            .map_or(0, |tz| i64::from(tz.get_time_zone_info(t.sse).offset)),
        ZoneType::Offset => i64::from(t.z),
        ZoneType::Abbr => i64::from(t.z) + 3600 * i64::from(t.dst),
        ZoneType::None => 0,
    }
}

/// `DateTime::setTime()` (`php_date_time_set`).
pub fn date_time_set(t: &mut Time, h: i64, i: i64, s: i64, us: i64) {
    t.h = h;
    t.i = i;
    t.s = s;
    t.us = us;
    t.update_ts(None);
    t.update_from_sse();
}

/// `DateTime::setDate()` (`php_date_date_set`).
pub fn date_date_set(t: &mut Time, y: i64, m: i64, d: i64) {
    t.y = y;
    t.m = m;
    t.d = d;
    t.update_ts(None);
}

/// `DateTime::setISODate()` (`php_date_isodate_set`).
pub fn date_isodate_set(t: &mut Time, y: i64, w: i64, d: i64) {
    t.y = y;
    t.m = 1;
    t.d = 1;
    t.relative = RelTime::default();
    t.relative.d = daynr_from_weeknr(y, w, d);
    t.have_relative = true;
    t.update_ts(None);
}

/// `DateTime::setTimestamp()` (`php_date_timestamp_set`).
pub fn date_timestamp_set(t: &mut Time, ts: i64) {
    t.unixtime2local(ts);
    t.update_ts(None);
    t.us = 0;
}

/// A `DateInterval` (`php_interval_obj`): the relative time plus whether it applies
/// with wall-clock (`new DateInterval`) or civil (`diff()`,
/// `createFromDateString()`) semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhpInterval {
    /// The interval.
    pub diff: RelTime,
    /// `true` for `PHP_DATE_WALL` (constructed from an ISO spec).
    pub wall: bool,
}

/// `new DateInterval($spec)` (`date_interval_initialize`). The error is PHP's
/// exception message.
pub fn date_interval_create(spec: &str) -> Result<PhpInterval, String> {
    let r = strtointerval(spec);
    if !r.errors.errors.is_empty() {
        return Err(format!("Unknown or bad format ({spec})"));
    }
    if let Some(p) = r.period {
        return Ok(PhpInterval {
            diff: p,
            wall: true,
        });
    }
    match (r.begin, r.end) {
        (Some(mut b), Some(mut e)) => {
            b.update_ts(None);
            e.update_ts(None);
            Ok(PhpInterval {
                diff: diff(&b, &e),
                wall: true,
            })
        }
        _ => Err(format!("Failed to parse interval ({spec})")),
    }
}

/// `DateInterval::createFromDateString($s)`: the relative part of a strtotime string
/// (civil semantics). `None` if the string does not parse.
pub fn date_interval_create_from_date_string(s: &str) -> Option<PhpInterval> {
    let res = strtotime(s, &builtin_tzinfo);
    if !res.errors.errors.is_empty() {
        return None;
    }
    Some(PhpInterval {
        diff: res.time.relative,
        wall: false,
    })
}

/// `DateTime::add()` (`php_date_add`).
pub fn date_add(t: &Time, interval: &PhpInterval) -> Time {
    if interval.wall {
        add_wall(t, &interval.diff)
    } else {
        add(t, &interval.diff)
    }
}

/// `DateTime::sub()` (`php_date_sub`); `None` for special relative intervals (PHP
/// warns/throws "Only non-special relative time specifications are supported for
/// subtraction").
pub fn date_sub(t: &Time, interval: &PhpInterval) -> Option<Time> {
    if interval.diff.have_weekday_relative || interval.diff.have_special_relative {
        return None;
    }
    Some(if interval.wall {
        sub_wall(t, &interval.diff)
    } else {
        sub(t, &interval.diff)
    })
}

/// `DateTime::diff($other, $absolute)` (`date_diff`).
pub fn date_diff(one: &Time, two: &Time, absolute: bool) -> PhpInterval {
    let mut d = diff(one, two);
    if absolute {
        d.invert = false;
    }
    PhpInterval {
        diff: d,
        wall: false,
    }
}

/// One entry of `DateTimeZone::getTransitions()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    /// Timestamp.
    pub ts: i64,
    /// UTC offset in seconds.
    pub offset: i32,
    /// DST flag.
    pub isdst: bool,
    /// Abbreviation.
    pub abbr: String,
}

impl Transition {
    /// The `time` element: `gmdate("x-m-d\TH:i:sP", ts)`.
    pub fn time(&self) -> String {
        String::from_utf8_lossy(&crate::format::format_timestamp(
            b"x-m-d\\TH:i:sP",
            self.ts,
            None,
        ))
        .into_owned()
    }
}

/// `DateTimeZone::getTransitions($begin, $end)` (`timezone_transitions_get`); PHP's
/// defaults are `i64::MIN` and `i32::MAX`.
pub fn timezone_transitions_get(
    tz: &TzInfo,
    timestamp_begin: i64,
    timestamp_end: i64,
) -> Vec<Transition> {
    let mut out = Vec::new();
    let by_type = |i: usize, ts: i64| {
        let t = tz.types.get(i).copied().unwrap_or_default();
        Transition {
            ts,
            offset: t.offset,
            isdst: t.isdst,
            abbr: tz.abbr_at(t.abbr_idx).to_string(),
        }
    };
    let by_trans = |i: usize, ts: i64| by_type(usize::from(tz.trans_idx[i]), ts);
    let timecnt = tz.trans.len();
    let mut begin = 0usize;
    let found = if timestamp_begin == i64::MIN {
        out.push(by_type(0, timestamp_begin));
        true
    } else {
        let mut f = false;
        while begin < timecnt {
            if tz.trans[begin] > timestamp_begin {
                if begin > 0 {
                    out.push(by_trans(begin - 1, timestamp_begin));
                } else {
                    out.push(by_type(0, timestamp_begin));
                }
                f = true;
                break;
            }
            begin += 1;
        }
        f
    };
    let has_dst_rule = tz.posix_info.as_ref().is_some_and(|p| p.dst_end.is_some());
    if !found {
        if timecnt > 0 {
            if has_dst_rule {
                let tto = tz.get_time_zone_info(timestamp_begin);
                out.push(Transition {
                    ts: timestamp_begin,
                    offset: tto.offset,
                    isdst: tto.is_dst,
                    abbr: tto.abbr,
                });
            } else {
                out.push(by_trans(timecnt - 1, timestamp_begin));
            }
        } else {
            out.push(by_type(0, timestamp_begin));
        }
    } else {
        for i in begin..timecnt {
            if tz.trans[i] < timestamp_end {
                out.push(by_trans(i, tz.trans[i]));
            } else {
                return out;
            }
        }
    }
    if has_dst_rule && timecnt > 0 {
        let last_transition_ts = tz.trans[timecnt - 1];
        let (start_y, _, _) = crate::unixtime2tm::unixtime2date(last_transition_ts);
        let (end_y, _, _) = crate::unixtime2tm::unixtime2date(timestamp_end);
        for year in start_y..=end_y {
            let tr = tz.transitions_for_year(year);
            for j in 0..tr.count {
                if tr.times[j] <= last_transition_ts || tr.times[j] < timestamp_begin {
                    continue;
                }
                if tr.times[j] > timestamp_end {
                    return out;
                }
                out.push(by_type(tr.types[j], tr.times[j]));
            }
        }
    }
    out
}
