//! Core data structures: [`Time`] (`timelib_time`), [`RelTime`] (`timelib_rel_time`)
//! and [`TimeOffset`] (`timelib_time_offset`).

use std::cmp::Ordering;
use std::sync::Arc;

use crate::tz::TzInfo;

/// Marker for "field not set" (`TIMELIB_UNSET`).
pub const UNSET: i64 = -9_999_999;

/// `TIMELIB_OVERRIDE_TIME` option for [`Time::fill_holes`].
pub const OVERRIDE_TIME: u32 = 0x01;
/// `TIMELIB_NO_CLONE` option for [`Time::fill_holes`] (kept for API parity; tz info is
/// reference counted so cloning is always cheap).
pub const NO_CLONE: u32 = 0x02;

/// `relative.special.type` values (`TIMELIB_SPECIAL_*`).
pub const SPECIAL_WEEKDAY: u32 = 0x01;
/// "first monday of ..." style special relative.
pub const SPECIAL_DAY_OF_WEEK_IN_MONTH: u32 = 0x02;
/// "last monday of ..." style special relative.
pub const SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH: u32 = 0x03;

/// `relative.first_last_day_of` values.
pub const SPECIAL_FIRST_DAY_OF_MONTH: i32 = 0x01;
/// "last day of" marker.
pub const SPECIAL_LAST_DAY_OF_MONTH: i32 = 0x02;

pub(crate) const SECS_PER_DAY: i64 = 86_400;
pub(crate) const SECS_PER_HOUR: i64 = 3_600;
pub(crate) const DAYS_PER_ERA: i64 = 146_097;
pub(crate) const YEARS_PER_ERA: i64 = 400;
pub(crate) const HINNANT_EPOCH_SHIFT: i64 = 719_468;

/// The kind of timezone information attached to a [`Time`] (`TIMELIB_ZONETYPE_*`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ZoneType {
    /// No zone information (`0`).
    #[default]
    None = 0,
    /// A plain UTC offset such as `+02:00` (`1`).
    Offset = 1,
    /// A timezone abbreviation such as `CEST` (`2`).
    Abbr = 2,
    /// A full timezone identifier such as `Europe/Amsterdam` (`3`).
    Id = 3,
}

impl ZoneType {
    /// Numeric value as used by PHP (`DateTimeZone::getLocation`, var_dump output, ...).
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// The "special" relative part (`timelib_rel_time.special`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpecialRelative {
    /// One of [`SPECIAL_WEEKDAY`], [`SPECIAL_DAY_OF_WEEK_IN_MONTH`],
    /// [`SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH`] or 0.
    pub kind: u32,
    /// The amount (e.g. number of weekdays).
    pub amount: i64,
}

/// A relative time / interval (`timelib_rel_time`); used both for the relative part of a
/// parsed string and for `DateInterval`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RelTime {
    /// Years.
    pub y: i64,
    /// Months.
    pub m: i64,
    /// Days.
    pub d: i64,
    /// Hours.
    pub h: i64,
    /// Minutes.
    pub i: i64,
    /// Seconds.
    pub s: i64,
    /// Microseconds.
    pub us: i64,
    /// Day of week for "next monday" style relatives (0 = Sunday; may be negative).
    pub weekday: i32,
    /// 0: the current day should not be counted when advancing forwards; 1: it should;
    /// 2: "this week" semantics.
    pub weekday_behavior: i32,
    /// [`SPECIAL_FIRST_DAY_OF_MONTH`], [`SPECIAL_LAST_DAY_OF_MONTH`] or 0.
    pub first_last_day_of: i32,
    /// Whether the interval is negative.
    pub invert: bool,
    /// Total number of days (from `diff`), or [`UNSET`].
    pub days: i64,
    /// Special relative ("weekday", "first monday of").
    pub special: SpecialRelative,
    /// Whether `weekday` is in use.
    pub have_weekday_relative: bool,
    /// Whether `special` is in use.
    pub have_special_relative: bool,
}

impl RelTime {
    /// Equivalent of `timelib_rel_time_ctor()` (all zero).
    pub fn new() -> Self {
        Self::default()
    }
}

/// UTC offset information for a timestamp in a zone (`timelib_time_offset`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimeOffset {
    /// UTC offset in seconds.
    pub offset: i32,
    /// Leap seconds (always 0 with PHP's bundled database).
    pub leap_secs: u32,
    /// Whether DST is in effect.
    pub is_dst: bool,
    /// Abbreviation, e.g. `CEST`.
    pub abbr: String,
    /// Start of the period this offset applies to (`i64::MIN` if unbounded).
    pub transition_time: i64,
}

/// Abbreviation information for [`Time::set_timezone_from_abbr`] (`timelib_abbr_info`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AbbrInfo {
    /// UTC offset in seconds (excluding the DST hour).
    pub utc_offset: i64,
    /// The abbreviation.
    pub abbr: String,
    /// DST flag (0 or 1).
    pub dst: i32,
}

/// A broken-down date/time with zone and relative information (`timelib_time`).
#[derive(Clone, Debug, Default)]
pub struct Time {
    /// Year.
    pub y: i64,
    /// Month.
    pub m: i64,
    /// Day.
    pub d: i64,
    /// Hour.
    pub h: i64,
    /// Minute.
    pub i: i64,
    /// Second.
    pub s: i64,
    /// Microsecond.
    pub us: i64,
    /// UTC offset in seconds (for offset/abbreviation zones excludes the DST hour).
    pub z: i32,
    /// Timezone abbreviation (display only).
    pub tz_abbr: Option<String>,
    /// Timezone structure for [`ZoneType::Id`].
    pub tz_info: Option<Arc<TzInfo>>,
    /// DST flag (may be [`UNSET`] after parsing).
    pub dst: i32,
    /// Relative part.
    pub relative: RelTime,
    /// Seconds since the Unix epoch.
    pub sse: i64,
    /// Time was set (a counter in the parser; non-zero means "have time").
    pub have_time: u32,
    /// Date was set.
    pub have_date: u32,
    /// Zone was set (a counter in the parser).
    pub have_zone: u32,
    /// Relative part is in use.
    pub have_relative: bool,
    /// Week number/day was set.
    pub have_weeknr_day: bool,
    /// `sse` is up to date with the broken-down fields.
    pub sse_uptodate: bool,
    /// Broken-down fields are up to date with `sse`.
    pub tim_uptodate: bool,
    /// Whether this represents local time (as opposed to UTC).
    pub is_localtime: bool,
    /// The kind of zone information.
    pub zone_type: ZoneType,
}

impl Time {
    /// Equivalent of `timelib_time_ctor()` (all zero).
    pub fn new() -> Self {
        Self::default()
    }

    /// A `Time` with all date/time fields set to [`UNSET`], as the parsers start with.
    pub(crate) fn new_unset() -> Self {
        Time {
            y: UNSET,
            m: UNSET,
            d: UNSET,
            h: UNSET,
            i: UNSET,
            s: UNSET,
            us: UNSET,
            z: UNSET as i32,
            dst: UNSET as i32,
            ..Default::default()
        }
    }

    /// Compares two times by `sse` and then microseconds (`timelib_time_compare`).
    pub fn compare(&self, other: &Time) -> Ordering {
        self.sse.cmp(&other.sse).then(self.us.cmp(&other.us))
    }

    /// Returns `sse` (`timelib_date_to_int`); `None` if it does not fit (never on 64-bit).
    pub fn date_to_int(&self) -> Option<i64> {
        Some(self.sse)
    }

    /// Sets the abbreviation, upper-casing it (`timelib_time_tz_abbr_update`).
    pub fn tz_abbr_update(&mut self, abbr: &str) {
        self.tz_abbr = Some(abbr.to_ascii_uppercase());
    }

    /// Fills unset fields from `now` (`timelib_fill_holes`).
    pub fn fill_holes(&mut self, now: &Time, options: u32) {
        if options & OVERRIDE_TIME == 0 && self.have_date != 0 && self.have_time == 0 {
            self.h = 0;
            self.i = 0;
            self.s = 0;
            self.us = 0;
        }
        if self.y != UNSET
            || self.m != UNSET
            || self.d != UNSET
            || self.h != UNSET
            || self.i != UNSET
            || self.s != UNSET
        {
            if self.us == UNSET {
                self.us = 0;
            }
        } else if self.us == UNSET {
            self.us = if now.us != UNSET { now.us } else { 0 };
        }
        let pick = |v: i64| if v != UNSET { v } else { 0 };
        if self.y == UNSET {
            self.y = pick(now.y);
        }
        if self.m == UNSET {
            self.m = pick(now.m);
        }
        if self.d == UNSET {
            self.d = pick(now.d);
        }
        if self.h == UNSET {
            self.h = pick(now.h);
        }
        if self.i == UNSET {
            self.i = pick(now.i);
        }
        if self.s == UNSET {
            self.s = pick(now.s);
        }

        if self.tz_info.is_none() {
            self.tz_info.clone_from(&now.tz_info);
            if self.z == UNSET as i32 {
                self.z = if now.z != UNSET as i32 { now.z } else { 0 };
            }
            if self.dst == UNSET as i32 {
                self.dst = if now.dst != UNSET as i32 { now.dst } else { 0 };
            }
            if self.tz_abbr.is_none() {
                self.tz_abbr.clone_from(&now.tz_abbr);
            }
        }

        if self.zone_type == ZoneType::None && now.zone_type != ZoneType::None {
            self.zone_type = now.zone_type;
            self.is_localtime = true;
        }
    }

    /// Current UTC offset including DST (`timelib_get_current_offset`).
    pub fn current_offset(&self) -> i64 {
        match self.zone_type {
            ZoneType::Abbr | ZoneType::Offset => {
                i64::from(self.z) + i64::from(self.dst) * SECS_PER_HOUR
            }
            ZoneType::Id => self
                .tz_info
                .as_ref()
                .and_then(|tz| tz.offset_info(self.sse))
                .map_or(0, |o| i64::from(o.offset)),
            ZoneType::None => 0,
        }
    }

    /// Whether both times are in the same timezone (`timelib_same_timezone`).
    pub fn same_timezone(&self, other: &Time) -> bool {
        if self.zone_type != other.zone_type {
            return false;
        }
        match self.zone_type {
            ZoneType::Abbr | ZoneType::Offset => {
                i64::from(self.z) + i64::from(self.dst) * 3600
                    == i64::from(other.z) + i64::from(other.dst) * 3600
            }
            ZoneType::Id => tz_name(self) == tz_name(other),
            ZoneType::None => false,
        }
    }

    /// The timezone identifier name, if this is an [`ZoneType::Id`] time.
    pub fn tz_name(&self) -> Option<&str> {
        self.tz_info.as_deref().map(|tz| tz.name.as_str())
    }
}

pub(crate) fn tz_name(t: &Time) -> &str {
    t.tz_info.as_deref().map_or("", |tz| tz.name.as_str())
}

/// Converts a decimal hour to `(hour, minute, second)` (`timelib_decimal_hour_to_hms`).
pub fn decimal_hour_to_hms(h: f64) -> (i32, i32, i32) {
    let swap = h < 0.0;
    let h = h.abs();
    let hour = h.floor() as i32;
    let seconds = ((h - f64::from(hour)) * 3600.0).floor() as i32;
    let min = seconds / 60;
    let sec = seconds % 60;
    (if swap { -hour } else { hour }, min, sec)
}

/// Converts hours/minutes/seconds to a decimal hour (`timelib_hms_to_decimal_hour`).
pub fn hms_to_decimal_hour(hour: i32, min: i32, sec: i32) -> f64 {
    if hour >= 0 {
        f64::from(hour) + f64::from(min) / 60.0 + f64::from(sec) / 3600.0
    } else {
        f64::from(hour) - f64::from(min) / 60.0 - f64::from(sec) / 3600.0
    }
}

/// Like [`hms_to_decimal_hour`] but including microseconds (`timelib_hmsf_to_decimal_hour`).
pub fn hmsf_to_decimal_hour(hour: i64, min: i64, sec: i64, us: i64) -> f64 {
    let (hour, min, sec, us) = (hour as i32, min as i32, sec as i32, us as i32);
    if hour >= 0 {
        (f64::from(hour) + f64::from(min) / 60.0 + f64::from(sec) / 3600.0)
            + f64::from(us) / 3_600_000_000.0
    } else {
        (f64::from(hour) - f64::from(min) / 60.0 - f64::from(sec) / 3600.0)
            - f64::from(us) / 3_600_000_000.0
    }
}

/// `h * 3600 + m * 60 + s` (`timelib_hms_to_seconds`).
pub fn hms_to_seconds(h: i64, m: i64, s: i64) -> i64 {
    h.wrapping_mul(SECS_PER_HOUR)
        .wrapping_add(m.wrapping_mul(60))
        .wrapping_add(s)
}
