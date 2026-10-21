//! Tests ported from php-src `ext/date/tests/*.phpt` (group 0).
//!
//! Each test names the phpt file it comes from. Parts of a phpt that need the real
//! current time, serialization, exceptions or class semantics are left out (noted in
//! the test's doc comment).
mod common;

use std::cmp::Ordering;
use std::fmt::Write as _;

use common::*;
use php_timelib::format::{MON_FULL_NAMES, full_day_name};
use php_timelib::php::{self, PhpInterval, PhpTimeZone, Transition};
use php_timelib::{
    ErrorContainer, SunFormat, SunValue, Time, ZoneType, builtin_db, day_of_week, day_of_year,
    sun_info, sunrise_sunset, valid_date,
};

/// `DATE_ISO8601` / `DateTime::ISO8601`.
const ISO8601: &str = "Y-m-d\\TH:i:sO";
/// `DATE_ATOM` / `DateTime::ATOM`.
const ATOM: &str = "Y-m-d\\TH:i:sP";

/// `strtotime($s)` for inputs that do not depend on "now" and must parse.
fn st(s: &str, zone: &str) -> i64 {
    strtotime_abs(s, zone).unwrap_or_else(|| panic!("strtotime({s:?}) returned false"))
}

/// An array key.
fn sk(s: &str) -> K {
    K::S(s.to_string())
}

/// `var_dump($string)`.
fn vd_str(s: &str) -> String {
    V::Str(s.to_string()).var_dump()
}

/// `var_dump($bool)`.
fn vd_bool(b: bool) -> String {
    V::Bool(b).var_dump()
}

/// `$array[$key]` (`None` if unset).
fn get<'a>(v: &'a V, key: &str) -> Option<&'a V> {
    match v {
        V::Arr(a) => a.iter().find(|(k, _)| *k == sk(key)).map(|(_, v)| v),
        _ => None,
    }
}

/// `var_dump($array[$key])` (`NULL` if unset).
fn vd_opt(v: Option<&V>) -> String {
    v.map_or_else(|| "NULL\n".to_string(), V::var_dump)
}

/// A time in `zone` after `timelib_unixtime2local($ts)`.
fn local(ts: i64, zone: &str) -> Time {
    let mut t = Time {
        tz_info: Some(tz(zone)),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    t.unixtime2local(ts);
    t
}

/// `getdate($ts)` with `date_default_timezone_set($zone)`.
fn getdate(ts: i64, zone: &str) -> V {
    let t = local(ts, zone);
    let (y, m, d) = (t.y, t.m, t.d);
    V::Arr(vec![
        (sk("seconds"), V::Int(t.s)),
        (sk("minutes"), V::Int(t.i)),
        (sk("hours"), V::Int(t.h)),
        (sk("mday"), V::Int(d)),
        (sk("wday"), V::Int(day_of_week(y, m, d))),
        (sk("mon"), V::Int(m)),
        (sk("year"), V::Int(y)),
        (sk("yday"), V::Int(day_of_year(y, m, d))),
        (sk("weekday"), V::Str(full_day_name(y, m, d).to_string())),
        (
            sk("month"),
            V::Str(MON_FULL_NAMES[(m - 1) as usize].to_string()),
        ),
        (K::I(0), V::Int(ts)),
    ])
}

/// `localtime($ts, $associative)` with `date_default_timezone_set($zone)`.
fn localtime(ts: i64, zone: &str, associative: bool) -> V {
    let t = local(ts, zone);
    let names = [
        "tm_sec", "tm_min", "tm_hour", "tm_mday", "tm_mon", "tm_year", "tm_wday", "tm_yday",
        "tm_isdst",
    ];
    let values = [
        t.s,
        t.i,
        t.h,
        t.d,
        t.m - 1,
        t.y - 1900,
        day_of_week(t.y, t.m, t.d),
        day_of_year(t.y, t.m, t.d),
        i64::from(t.dst),
    ];
    V::Arr(
        names
            .iter()
            .zip(values)
            .enumerate()
            .map(|(i, (n, v))| {
                let k = if associative { sk(n) } else { K::I(i as i64) };
                (k, V::Int(v))
            })
            .collect(),
    )
}

/// `checkdate($m, $d, $y)`.
fn checkdate(m: i64, d: i64, y: i64) -> bool {
    (1..=32767).contains(&y) && valid_date(y, m, d)
}

/// A `DateTimeZone` object (`print_r`/`var_dump` view).
fn tz_obj(z: &PhpTimeZone, id: u32) -> V {
    V::Obj(
        "DateTimeZone".into(),
        id,
        vec![
            (
                sk("timezone_type"),
                V::Int(i64::from(z.zone_type().as_i32())),
            ),
            (sk("timezone"), V::Str(z.name())),
        ],
    )
}

/// One element of `DateTimeZone::getTransitions()`.
fn transition_v(t: &Transition) -> V {
    V::Arr(vec![
        (sk("ts"), V::Int(t.ts)),
        (sk("time"), V::Str(t.time())),
        (sk("offset"), V::Int(i64::from(t.offset))),
        (sk("isdst"), V::Bool(t.isdst)),
        (sk("abbr"), V::Str(t.abbr.clone())),
    ])
}

/// `timezone_identifiers_list()` (`DateTimeZone::ALL`): canonical identifiers in the
/// continent groups plus UTC (`check_id_allowed`).
fn identifiers_list() -> Vec<String> {
    const GROUPS: [&str; 11] = [
        "africa/",
        "america/",
        "antarctica/",
        "arctic/",
        "asia/",
        "atlantic/",
        "australia/",
        "europe/",
        "indian/",
        "pacific/",
        "utc",
    ];
    let db = builtin_db();
    db.identifiers()
        .iter()
        .filter(|e| {
            let id = e.id.to_ascii_lowercase();
            db.entry_is_bc(e) && GROUPS.iter().any(|g| id.starts_with(g))
        })
        .map(|e| e.id.clone())
        .collect()
}

/// PHP's `"<prefix> (<input>) at position %d (%c)<mid>: <message>"` error text for
/// the first parse error.
fn parse_error(prefix: &str, input: &str, mid: &str, e: &ErrorContainer) -> String {
    let m = &e.errors[0];
    let c = if m.character == 0 {
        ' '
    } else {
        char::from(m.character)
    };
    format!(
        "{prefix} ({input}) at position {} ({c}){mid}: {}",
        m.position, m.message
    )
}

/// `date_period_advance()`: one DatePeriod iteration step.
fn period_advance(t: &mut Time, interval: &PhpInterval) {
    t.have_relative = true;
    t.relative = interval.diff;
    t.sse_uptodate = false;
    t.update_ts(None);
    t.update_from_sse();
}

/// Which part of `examine_diff.inc` a test shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Diff,
    Days,
    Add,
    Sub,
}

/// `examine_diff.inc` driven by one of the `DateTime_data-*.inc` tables below
/// (all of which use `date_default_timezone_set('America/New_York')`).
fn examine_diff(data: &str, show: Show) -> String {
    let zone = "America/New_York";
    let mk = |s: &str| {
        let (s, set_tz) = match s.strip_suffix('*') {
            Some(s) => (s, true),
            None => (s, false),
        };
        let mut d = DT::new(s, zone);
        if set_tz {
            d.set_timezone(zone);
        }
        d
    };
    let mut out = String::new();
    for line in data.lines().filter(|l| !l.is_empty()) {
        let f: Vec<&str> = line.split('|').collect();
        let (name, mut end, mut start, spec) = (f[0], mk(f[1]), mk(f[2]), f[3]);
        let start_date = start.format("Y-m-d H:i:s T");
        let end_date = end.format("Y-m-d H:i:s T");
        let mut expect_interval = php::date_interval_create(&format!("P{}", &spec[2..])).unwrap();
        if &spec[1..2] == "-" {
            expect_interval.diff.invert = true;
        }
        let _ = write!(out, "{name}: ");
        match show {
            Show::Diff => {
                let r = interval_format(&start.diff(&end), "P%R%yY%mM%dDT%hH%iM%sS");
                let _ = writeln!(out, "DIFF: {end_date} - {start_date} = **{r}**");
            }
            Show::Days => {
                let r = interval_format(&start.diff(&end), "%a");
                let _ = writeln!(out, "DAYS: **{r}**");
            }
            Show::Add => {
                start.add_interval(&expect_interval);
                let r = start.format("Y-m-d H:i:s T");
                let _ = writeln!(out, "ADD: {start_date} + {spec} = **{r}**");
            }
            Show::Sub => {
                end.sub_interval(&expect_interval);
                let r = end.format("Y-m-d H:i:s T");
                let _ = writeln!(out, "SUB: {end_date} - {spec} = **{r}**");
            }
        }
    }
    out
}

/// `DateTime_data-fall-type2-type2.inc`: `name|end|start|spec`; a trailing `*` means the
/// object is then `setTimezone(new DateTimeZone('America/New_York'))`.
const DATA_FALL_TYPE2_TYPE2: &str = "
test_time_fall_type2_prev_type2_prev|2010-11-06 18:38:28 EDT|2010-10-04 02:18:48 EDT|P+0Y1M2DT16H19M40S
test_time_fall_type2_prev_type2_dt|2010-11-07 00:10:20 EDT|2010-11-06 18:38:28 EDT|P+0Y0M0DT5H31M52S
test_time_fall_type2_prev_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-06 18:38:28 EDT|P+0Y0M0DT6H34M5S
test_time_fall_type2_prev_type2_redost|2010-11-07 01:14:44 EST|2010-11-06 18:38:28 EDT|P+0Y0M0DT7H36M16S
test_time_fall_type2_prev_type2_st|2010-11-07 03:16:55 EST|2010-11-06 18:38:28 EDT|P+0Y0M0DT9H38M27S
test_time_fall_type2_prev_type2_post|2010-11-08 19:59:59 EST|2010-11-06 18:38:28 EDT|P+0Y0M2DT1H21M31S
test_time_fall_type2_dt_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 00:10:20 EDT|P-0Y0M0DT5H31M52S
test_time_fall_type2_dt_type2_dt|2010-11-07 00:15:35 EDT|2010-11-07 00:10:20 EDT|P+0Y0M0DT0H5M15S
test_time_fall_type2_dt_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-07 00:10:20 EDT|P+0Y0M0DT1H2M13S
test_time_fall_type2_dt_type2_redost|2010-11-07 01:14:44 EST|2010-11-07 00:10:20 EDT|P+0Y0M0DT2H4M24S
test_time_fall_type2_dt_type2_st|2010-11-07 03:16:55 EST|2010-11-07 00:10:20 EDT|P+0Y0M0DT4H6M35S
test_time_fall_type2_dt_type2_post|2010-11-08 19:59:59 EST|2010-11-07 00:10:20 EDT|P+0Y0M1DT20H49M39S
test_time_fall_type2_redodt_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 01:12:33 EDT|P-0Y0M0DT6H34M5S
test_time_fall_type2_redodt_type2_dt|2010-11-07 00:10:20 EDT|2010-11-07 01:12:33 EDT|P-0Y0M0DT1H2M13S
test_time_fall_type2_redodt_type2_redodt|2010-11-07 01:15:35 EDT|2010-11-07 01:12:33 EDT|P+0Y0M0DT0H3M2S
test_time_fall_type2_redodt_type2_redost|2010-11-07 01:14:44 EST|2010-11-07 01:12:33 EDT|P+0Y0M0DT1H2M11S
test_time_fall_type2_redodt_type2_st|2010-11-07 03:16:55 EST|2010-11-07 01:12:33 EDT|P+0Y0M0DT3H4M22S
test_time_fall_type2_redodt_type2_post|2010-11-08 19:59:59 EST|2010-11-07 01:12:33 EDT|P+0Y0M1DT19H47M26S
test_time_fall_type2_redost_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 01:14:44 EST|P-0Y0M0DT7H36M16S
test_time_fall_type2_redost_type2_dt|2010-11-07 00:10:20 EDT|2010-11-07 01:14:44 EST|P-0Y0M0DT2H4M24S
test_time_fall_type2_redost_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-07 01:14:44 EST|P-0Y0M0DT1H2M11S
test_time_fall_type2_redost_type2_redost|2010-11-07 01:16:54 EST|2010-11-07 01:14:44 EST|P+0Y0M0DT0H2M10S
test_time_fall_type2_redost_type2_st|2010-11-07 03:16:55 EST|2010-11-07 01:14:44 EST|P+0Y0M0DT2H2M11S
test_time_fall_type2_redost_type2_post|2010-11-08 19:59:59 EST|2010-11-07 01:14:44 EST|P+0Y0M1DT18H45M15S
test_time_fall_type2_st_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 03:16:55 EST|P-0Y0M0DT9H38M27S
test_time_fall_type2_st_type2_dt|2010-11-07 00:10:20 EDT|2010-11-07 03:16:55 EST|P-0Y0M0DT4H6M35S
test_time_fall_type2_st_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-07 03:16:55 EST|P-0Y0M0DT3H4M22S
test_time_fall_type2_st_type2_redost|2010-11-07 01:14:44 EST|2010-11-07 03:16:55 EST|P-0Y0M0DT2H2M11S
test_time_fall_type2_st_type2_st|2010-11-07 05:19:56 EST|2010-11-07 03:16:55 EST|P+0Y0M0DT2H3M1S
test_time_fall_type2_st_type2_post|2010-11-08 19:59:59 EST|2010-11-07 03:16:55 EST|P+0Y0M1DT16H43M4S
test_time_fall_type2_post_type2_prev|2010-11-06 18:38:28 EDT|2010-11-08 19:59:59 EST|P-0Y0M2DT1H21M31S
test_time_fall_type2_post_type2_dt|2010-11-07 00:10:20 EDT|2010-11-08 19:59:59 EST|P-0Y0M1DT20H49M39S
test_time_fall_type2_post_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-08 19:59:59 EST|P-0Y0M1DT19H47M26S
test_time_fall_type2_post_type2_redost|2010-11-07 01:14:44 EST|2010-11-08 19:59:59 EST|P-0Y0M1DT18H45M15S
test_time_fall_type2_post_type2_st|2010-11-07 03:16:55 EST|2010-11-08 19:59:59 EST|P-0Y0M1DT16H43M4S
test_time_fall_type2_post_type2_post|2010-11-08 19:59:59 EST|2010-11-08 18:57:55 EST|P+0Y0M0DT1H2M4S
test_time_fall_type2_dtsec_type2_stsec|2010-11-07 01:00:00 EST|2010-11-07 01:59:59 EDT|P+0Y0M0DT0H0M1S
test_time_fall_type2_stsec_type2_dtsec|2010-11-07 01:59:59 EDT|2010-11-07 01:00:00 EST|P-0Y0M0DT0H0M1S
";

/// `DateTime_data-spring-type2-type2.inc`: `name|end|start|spec`; a trailing `*` means the
/// object is then `setTimezone(new DateTimeZone('America/New_York'))`.
const DATA_SPRING_TYPE2_TYPE2: &str = "
test_time_spring_type2_prev_type2_prev|2010-03-13 18:38:28 EST|2010-02-11 02:18:48 EST|P+0Y1M2DT16H19M40S
test_time_spring_type2_prev_type2_st|2010-03-14 00:10:20 EST|2010-03-13 18:38:28 EST|P+0Y0M0DT5H31M52S
test_time_spring_type2_prev_type2_dt|2010-03-14 03:16:55 EDT|2010-03-13 18:38:28 EST|P+0Y0M0DT7H38M27S
test_time_spring_type2_prev_type2_post|2010-03-15 19:59:59 EDT|2010-03-13 18:38:28 EST|P+0Y0M2DT1H21M31S
test_time_spring_type2_st_type2_prev|2010-03-13 18:38:28 EST|2010-03-14 00:10:20 EST|P-0Y0M0DT5H31M52S
test_time_spring_type2_st_type2_st|2010-03-14 00:15:35 EST|2010-03-14 00:10:20 EST|P+0Y0M0DT0H5M15S
test_time_spring_type2_st_type2_dt|2010-03-14 03:16:55 EDT|2010-03-14 00:10:20 EST|P+0Y0M0DT2H6M35S
test_time_spring_type2_st_type2_post|2010-03-15 19:59:59 EDT|2010-03-14 00:10:20 EST|P+0Y0M1DT18H49M39S
test_time_spring_type2_dt_type2_prev|2010-03-13 18:38:28 EST|2010-03-14 03:16:55 EDT|P-0Y0M0DT7H38M27S
test_time_spring_type2_dt_type2_st|2010-03-14 00:10:20 EST|2010-03-14 03:16:55 EDT|P-0Y0M0DT2H6M35S
test_time_spring_type2_dt_type2_dt|2010-03-14 05:19:56 EDT|2010-03-14 03:16:55 EDT|P+0Y0M0DT2H3M1S
test_time_spring_type2_dt_type2_post|2010-03-15 19:59:59 EDT|2010-03-14 03:16:55 EDT|P+0Y0M1DT16H43M4S
test_time_spring_type2_post_type2_prev|2010-03-13 18:38:28 EST|2010-03-15 19:59:59 EDT|P-0Y0M2DT1H21M31S
test_time_spring_type2_post_type2_st|2010-03-14 00:10:20 EST|2010-03-15 19:59:59 EDT|P-0Y0M1DT18H49M39S
test_time_spring_type2_post_type2_dt|2010-03-14 03:16:55 EDT|2010-03-15 19:59:59 EDT|P-0Y0M1DT16H43M4S
test_time_spring_type2_post_type2_post|2010-03-15 19:59:59 EDT|2010-03-15 18:57:55 EDT|P+0Y0M0DT1H2M4S
test_time_spring_type2_stsec_type2_dtsec|2010-03-14 03:00:00 EDT|2010-03-14 01:59:59 EST|P+0Y0M0DT0H0M1S
test_time_spring_type2_dtsec_type2_stsec|2010-03-14 01:59:59 EST|2010-03-14 03:00:00 EDT|P-0Y0M0DT0H0M1S
";

/// `DateTime_data-fall-type3-type2.inc`: `name|end|start|spec`; a trailing `*` means the
/// object is then `setTimezone(new DateTimeZone('America/New_York'))`.
const DATA_FALL_TYPE3_TYPE2: &str = "
test_time_fall_type3_prev_type2_prev|2010-11-06 18:38:28 EDT|2010-10-04 02:18:48|P+0Y1M2DT16H19M40S
test_time_fall_type3_prev_type2_dt|2010-11-07 00:10:20 EDT|2010-11-06 18:38:28|P+0Y0M0DT5H31M52S
test_time_fall_type3_prev_type2_redodt|2010-11-07 01:12:33 EDT*|2010-11-06 18:38:28|P+0Y0M0DT6H34M5S
test_time_fall_type3_prev_type2_redost|2010-11-07 01:14:44 EST*|2010-11-06 18:38:28|P+0Y0M0DT7H36M16S
test_time_fall_type3_prev_type2_st|2010-11-07 03:16:55 EST|2010-11-06 18:38:28|P+0Y0M0DT9H38M27S
test_time_fall_type3_prev_type2_post|2010-11-08 19:59:59 EST|2010-11-06 18:38:28|P+0Y0M2DT1H21M31S
test_time_fall_type3_dt_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 00:10:20|P-0Y0M0DT5H31M52S
test_time_fall_type3_dt_type2_dt|2010-11-07 00:15:35 EDT|2010-11-07 00:10:20|P+0Y0M0DT0H5M15S
test_time_fall_type3_dt_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-07 00:10:20|P+0Y0M0DT1H2M13S
test_time_fall_type3_dt_type2_redost|2010-11-07 01:14:44 EST|2010-11-07 00:10:20|P+0Y0M0DT2H4M24S
test_time_fall_type3_dt_type2_st|2010-11-07 03:16:55 EST|2010-11-07 00:10:20|P+0Y0M0DT4H6M35S
test_time_fall_type3_dt_type2_post|2010-11-08 19:59:59 EST|2010-11-07 00:10:20|P+0Y0M1DT20H49M39S
test_time_fall_type3_redodt_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 01:12:33 EDT*|P-0Y0M0DT6H34M5S
test_time_fall_type3_redodt_type2_dt|2010-11-07 00:10:20 EDT|2010-11-07 01:12:33 EDT*|P-0Y0M0DT1H2M13S
test_time_fall_type3_redodt_type2_redodt|2010-11-07 01:15:35 EDT|2010-11-07 01:12:33 EDT*|P+0Y0M0DT0H3M2S
test_time_fall_type3_redodt_type2_redost|2010-11-07 01:14:44 EST|2010-11-07 01:12:33 EDT*|P+0Y0M0DT1H2M11S
test_time_fall_type3_redodt_type2_st|2010-11-07 03:16:55 EST|2010-11-07 01:12:33 EDT*|P+0Y0M0DT3H4M22S
test_time_fall_type3_redodt_type2_post|2010-11-08 19:59:59 EST|2010-11-07 01:12:33 EDT*|P+0Y0M1DT19H47M26S
test_time_fall_type3_redost_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 01:14:44 EST*|P-0Y0M0DT7H36M16S
test_time_fall_type3_redost_type2_dt|2010-11-07 00:10:20 EDT|2010-11-07 01:14:44 EST*|P-0Y0M0DT2H4M24S
test_time_fall_type3_redost_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-07 01:14:44 EST*|P-0Y0M0DT1H2M11S
test_time_fall_type3_redost_type2_redost|2010-11-07 01:16:54 EST|2010-11-07 01:14:44 EST*|P+0Y0M0DT0H2M10S
test_time_fall_type3_redost_type2_st|2010-11-07 03:16:55 EST|2010-11-07 01:14:44 EST*|P+0Y0M0DT2H2M11S
test_time_fall_type3_redost_type2_post|2010-11-08 19:59:59 EST|2010-11-07 01:14:44 EST*|P+0Y0M1DT18H45M15S
test_time_fall_type3_st_type2_prev|2010-11-06 18:38:28 EDT|2010-11-07 03:16:55|P-0Y0M0DT9H38M27S
test_time_fall_type3_st_type2_dt|2010-11-07 00:10:20 EDT|2010-11-07 03:16:55|P-0Y0M0DT4H6M35S
test_time_fall_type3_st_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-07 03:16:55|P-0Y0M0DT3H4M22S
test_time_fall_type3_st_type2_redost|2010-11-07 01:14:44 EST|2010-11-07 03:16:55|P-0Y0M0DT2H2M11S
test_time_fall_type3_st_type2_st|2010-11-07 05:19:56 EST|2010-11-07 03:16:55|P+0Y0M0DT2H3M1S
test_time_fall_type3_st_type2_post|2010-11-08 19:59:59 EST|2010-11-07 03:16:55|P+0Y0M1DT16H43M4S
test_time_fall_type3_post_type2_prev|2010-11-06 18:38:28 EDT|2010-11-08 19:59:59|P-0Y0M2DT1H21M31S
test_time_fall_type3_post_type2_dt|2010-11-07 00:10:20 EDT|2010-11-08 19:59:59|P-0Y0M1DT20H49M39S
test_time_fall_type3_post_type2_redodt|2010-11-07 01:12:33 EDT|2010-11-08 19:59:59|P-0Y0M1DT19H47M26S
test_time_fall_type3_post_type2_redost|2010-11-07 01:14:44 EST|2010-11-08 19:59:59|P-0Y0M1DT18H45M15S
test_time_fall_type3_post_type2_st|2010-11-07 03:16:55 EST|2010-11-08 19:59:59|P-0Y0M1DT16H43M4S
test_time_fall_type3_post_type2_post|2010-11-08 19:59:59 EST|2010-11-08 18:57:55|P+0Y0M0DT1H2M4S
test_time_fall_type3_dtsec_type2_stsec|2010-11-07 01:00:00 EST|2010-11-07 01:59:59 EDT*|P+0Y0M0DT0H0M1S
test_time_fall_type3_stsec_type2_dtsec|2010-11-07 01:59:59 EDT|2010-11-07 01:00:00 EST*|P-0Y0M0DT0H0M1S
";

/// `DateTime_data-spring-type3-type2.inc`: `name|end|start|spec`; a trailing `*` means the
/// object is then `setTimezone(new DateTimeZone('America/New_York'))`.
const DATA_SPRING_TYPE3_TYPE2: &str = "
test_time_spring_type3_prev_type2_prev|2010-03-13 18:38:28 EST|2010-02-11 02:18:48|P+0Y1M2DT16H19M40S
test_time_spring_type3_prev_type2_st|2010-03-14 00:10:20 EST|2010-03-13 18:38:28|P+0Y0M0DT5H31M52S
test_time_spring_type3_prev_type2_dt|2010-03-14 03:16:55 EDT|2010-03-13 18:38:28|P+0Y0M0DT7H38M27S
test_time_spring_type3_prev_type2_post|2010-03-15 19:59:59 EDT|2010-03-13 18:38:28|P+0Y0M2DT1H21M31S
test_time_spring_type3_st_type2_prev|2010-03-13 18:38:28 EST|2010-03-14 00:10:20|P-0Y0M0DT5H31M52S
test_time_spring_type3_st_type2_st|2010-03-14 00:15:35 EST|2010-03-14 00:10:20|P+0Y0M0DT0H5M15S
test_time_spring_type3_st_type2_dt|2010-03-14 03:16:55 EDT|2010-03-14 00:10:20|P+0Y0M0DT2H6M35S
test_time_spring_type3_st_type2_post|2010-03-15 19:59:59 EDT|2010-03-14 00:10:20|P+0Y0M1DT18H49M39S
test_time_spring_type3_dt_type2_prev|2010-03-13 18:38:28 EST|2010-03-14 03:16:55|P-0Y0M0DT7H38M27S
test_time_spring_type3_dt_type2_st|2010-03-14 00:10:20 EST|2010-03-14 03:16:55|P-0Y0M0DT2H6M35S
test_time_spring_type3_dt_type2_dt|2010-03-14 05:19:56 EDT|2010-03-14 03:16:55|P+0Y0M0DT2H3M1S
test_time_spring_type3_dt_type2_post|2010-03-15 19:59:59 EDT|2010-03-14 03:16:55|P+0Y0M1DT16H43M4S
test_time_spring_type3_post_type2_prev|2010-03-13 18:38:28 EST|2010-03-15 19:59:59|P-0Y0M2DT1H21M31S
test_time_spring_type3_post_type2_st|2010-03-14 00:10:20 EST|2010-03-15 19:59:59|P-0Y0M1DT18H49M39S
test_time_spring_type3_post_type2_dt|2010-03-14 03:16:55 EDT|2010-03-15 19:59:59|P-0Y0M1DT16H43M4S
test_time_spring_type3_post_type2_post|2010-03-15 19:59:59 EDT|2010-03-15 18:57:55|P+0Y0M0DT1H2M4S
test_time_spring_type3_stsec_type2_dtsec|2010-03-14 03:00:00 EDT|2010-03-14 01:59:59|P+0Y0M0DT0H0M1S
test_time_spring_type3_dtsec_type2_stsec|2010-03-14 01:59:59 EST|2010-03-14 03:00:00|P-0Y0M0DT0H0M1S
";

/// ext/date/tests/002.phpt
#[test]
fn test_002() {
    let dates = [
        "1999-10-13",
        "Oct 13  1999",
        "2000-01-19",
        "Jan 19  2000",
        "2001-12-21",
        "Dec 21  2001",
        "2001-12-21 12:16",
        "Dec 21 2001 12:16",
        "Dec 21  12:16",
        "2001-10-22 21:19:58",
        "2001-10-22 21:19:58-02",
        "2001-10-22 21:19:58-0213",
        "2001-10-22 21:19:58+02",
        "2001-10-22 21:19:58+0213",
        "2001-10-22T21:20:58-03:40",
        "2001-10-22T211958-2",
        "20011022T211958+0213",
        "20011022T21:20+0215",
        "1997W011",
        "2004W101T05:00+0",
    ];
    let mut out = String::new();
    for zone in ["GMT0", "US/Eastern"] {
        let _ = writeln!(out, "*** {zone}");
        for d in dates {
            out += &date("Y-m-d H:i:s\n", st(d, zone), zone);
        }
    }
    // "Dec 21  12:16" takes its year from "now" (common::NOW is in 2005).
    assert_text(
        &out,
        &r#"*** GMT0
1999-10-13 00:00:00
1999-10-13 00:00:00
2000-01-19 00:00:00
2000-01-19 00:00:00
2001-12-21 00:00:00
2001-12-21 00:00:00
2001-12-21 12:16:00
2001-12-21 12:16:00
%d-12-21 12:16:00
2001-10-22 21:19:58
2001-10-22 23:19:58
2001-10-22 23:32:58
2001-10-22 19:19:58
2001-10-22 19:06:58
2001-10-23 01:00:58
2001-10-22 23:19:58
2001-10-22 19:06:58
2001-10-22 19:05:00
1996-12-30 00:00:00
2004-03-01 05:00:00
*** US/Eastern
1999-10-13 00:00:00
1999-10-13 00:00:00
2000-01-19 00:00:00
2000-01-19 00:00:00
2001-12-21 00:00:00
2001-12-21 00:00:00
2001-12-21 12:16:00
2001-12-21 12:16:00
%d-12-21 12:16:00
2001-10-22 21:19:58
2001-10-22 19:19:58
2001-10-22 19:32:58
2001-10-22 15:19:58
2001-10-22 15:06:58
2001-10-22 21:00:58
2001-10-22 19:19:58
2001-10-22 15:06:58
2001-10-22 15:05:00
1996-12-30 00:00:00
2004-03-01 00:00:00"#
            .replace("%d-12-21", "2005-12-21"),
    );
}

/// ext/date/tests/008.phpt (partial: `getdate()` without a timestamp is not checked)
#[test]
fn test_008() {
    let t = mktime(0, 0, 0, 6, 27, 2006, "UTC");
    assert_text(
        &getdate(t, "UTC").var_dump(),
        r#"array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(0)
  ["mday"]=>
  int(27)
  ["wday"]=>
  int(2)
  ["mon"]=>
  int(6)
  ["year"]=>
  int(2006)
  ["yday"]=>
  int(177)
  ["weekday"]=>
  string(7) "Tuesday"
  ["month"]=>
  string(4) "June"
  [0]=>
  int(1151366400)
}"#,
    );
}

/// ext/date/tests/013.phpt
#[test]
fn test_013() {
    let mut dto = DT::new("2006-12-12", "UTC");
    let mut out = dto.var_dump("DateTime", 1);
    out += &vd_str(&dto.format("Y.m.d H:i:s"));
    dto.set_date(2006, 2, 15);
    out += &dto.var_dump("DateTime", 1);
    out += &vd_str(&dto.format("Y.m.d H:i:s"));
    dto.set_date(2006, 24, 60);
    out += &dto.var_dump("DateTime", 1);
    out += &vd_str(&dto.format("Y.m.d H:i:s"));
    out += "Done\n";
    assert_text(
        &out,
        &r#"object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2006-12-12 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
string(19) "2006.12.12 00:00:00"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2006-02-15 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
string(19) "2006.02.15 00:00:00"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2008-01-29 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
string(19) "2008.01.29 00:00:00"
Done"#
            .replace("#%d", "#1"),
    );
}

/// ext/date/tests/DateInterval_days_prop1.phpt
#[test]
fn dateinterval_days_prop1() {
    let interval = php::date_interval_create("P2Y4DT6H8M").unwrap();
    assert_eq!(interval_days(&interval.diff), None);
}

/// ext/date/tests/DatePeriod_serialize-001.phpt (partial: the original object's
/// start, interval and recurrences; serialization is not ported)
#[test]
fn dateperiod_serialize_001() {
    let r = php_timelib::strtointerval("R4/2012-07-01T00:00:00Z/P7D");
    assert!(r.errors.errors.is_empty());
    let mut start = r.begin.expect("start");
    start.update_ts(None);
    let start = DT {
        t: start,
        default_zone: "Europe/London".into(),
    };
    assert_text(
        &start.var_dump("DateTimeImmutable", 2),
        r#"object(DateTimeImmutable)#2 (3) {
  ["date"]=>
  string(26) "2012-07-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}"#,
    );
    assert!(r.end.is_none());
    let interval = PhpInterval {
        diff: r.period.expect("period"),
        wall: true,
    };
    assert_text(
        &interval_var_dump(&interval, 3),
        r#"object(DateInterval)#3 (10) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(7)
  ["h"]=>
  int(0)
  ["i"]=>
  int(0)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  bool(false)
  ["from_string"]=>
  bool(false)
}"#,
    );
    // DatePeriod's "recurrences" property is R + include_start_date.
    assert_eq!(r.recurrences.map(|n| n + 1), Some(5));
}

/// ext/date/tests/DateTimeImmutable_createFromInterface-002.phpt (partial: the
/// subclass is only reflected in the class name)
#[test]
fn datetimeimmutable_createfrominterface_002() {
    let mut out = String::new();
    for current in [
        "2014-03-02 16:24:08",
        "2019-12-16 15:06:46 CET",
        "2019-12-16 15:08:20 +0100",
    ] {
        for _ in 0..2 {
            out += &DT::new(current, "Europe/London").var_dump("MyDateTimeImmutable", 1);
        }
    }
    assert_text(
        &out,
        &r#"object(MyDateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(MyDateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(MyDateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(MyDateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(MyDateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}
object(MyDateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}"#
        .replace("#%d", "#1"),
    );
}

/// ext/date/tests/DateTimeImmutable_modify_invalid_format.phpt
#[test]
fn datetimeimmutable_modify_invalid_format() {
    let mut datetime = DT::new("now", "UTC");
    let e = php::date_modify(&mut datetime.t, b"").unwrap_err();
    let msg = parse_error(
        "DateTimeImmutable::modify(): Failed to parse time string",
        "",
        "",
        &e,
    );
    assert_text(
        &format!("DateMalformedStringException: {msg}"),
        r#"DateMalformedStringException: DateTimeImmutable::modify(): Failed to parse time string () at position 0 ( ): Empty string"#,
    );
}

/// ext/date/tests/DateTimeZone_getName_basic1.phpt
#[test]
fn datetimezone_getname_basic1() {
    let mut out = String::from("*** Testing DateTimeZone::getName() : basic functionality ***\n");
    for z in ["Europe/London", "America/New_York", "America/Los_Angeles"] {
        out += &vd_str(&php::timezone_open(z).unwrap().name());
    }
    assert_text(
        &out,
        r#"*** Testing DateTimeZone::getName() : basic functionality ***
string(13) "Europe/London"
string(16) "America/New_York"
string(19) "America/Los_Angeles""#,
    );
}

/// ext/date/tests/DateTimeZone_listIdentifiers_basic1.phpt
#[test]
fn datetimezone_listidentifiers_basic1() {
    let zones = identifiers_list();
    for z in ["Europe/London", "America/New_York", "UTC"] {
        assert!(zones.iter().any(|x| x == z), "{z} missing");
    }
}

/// ext/date/tests/DateTime_add-fall-type3-type2.phpt
#[test]
fn datetime_add_fall_type3_type2() {
    assert_text(
        &examine_diff(DATA_FALL_TYPE3_TYPE2, Show::Add),
        r#"test_time_fall_type3_prev_type2_prev: ADD: 2010-10-04 02:18:48 EDT + P+0Y1M2DT16H19M40S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type2_dt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_prev_type2_redodt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_prev_type2_redost: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT7H36M16S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_prev_type2_st: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT9H38M27S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_prev_type2_post: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_dt_type2_prev: ADD: 2010-11-07 00:10:20 EDT + P-0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_dt_type2_dt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT0H5M15S = **2010-11-07 00:15:35 EDT**
test_time_fall_type3_dt_type2_redodt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_dt_type2_redost: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT2H4M24S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_dt_type2_st: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT4H6M35S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_dt_type2_post: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EST**
test_time_fall_type3_redodt_type2_prev: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_redodt_type2_dt: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_redodt_type2_redodt: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT0H3M2S = **2010-11-07 01:15:35 EDT**
test_time_fall_type3_redodt_type2_redost: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT1H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redodt_type2_st: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT3H4M22S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_redodt_type2_post: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EST**
test_time_fall_type3_redost_type2_prev: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT7H36M16S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_redost_type2_dt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT2H4M24S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_redost_type2_redodt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT1H2M11S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redost_type2_redost: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT0H2M10S = **2010-11-07 01:16:54 EST**
test_time_fall_type3_redost_type2_st: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_redost_type2_post: ADD: 2010-11-07 01:14:44 EST + P+0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_st_type2_prev: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT9H38M27S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_st_type2_dt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT4H6M35S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_st_type2_redodt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT3H4M22S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_st_type2_redost: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_st_type2_st: ADD: 2010-11-07 03:16:55 EST + P+0Y0M0DT2H3M1S = **2010-11-07 05:19:56 EST**
test_time_fall_type3_st_type2_post: ADD: 2010-11-07 03:16:55 EST + P+0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_post_type2_prev: ADD: 2010-11-08 19:59:59 EST + P-0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_post_type2_dt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT20H49M39S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_post_type2_redodt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT19H47M26S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_post_type2_redost: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_post_type2_st: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_post_type2_post: ADD: 2010-11-08 18:57:55 EST + P+0Y0M0DT1H2M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_dtsec_type2_stsec: ADD: 2010-11-07 01:59:59 EDT + P+0Y0M0DT0H0M1S = **2010-11-07 01:00:00 EST**
test_time_fall_type3_stsec_type2_dtsec: ADD: 2010-11-07 01:00:00 EST + P-0Y0M0DT0H0M1S = **2010-11-07 01:59:59 EDT**"#,
    );
}

/// ext/date/tests/DateTime_add-spring-type3-type2.phpt
#[test]
fn datetime_add_spring_type3_type2() {
    assert_text(
        &examine_diff(DATA_SPRING_TYPE3_TYPE2, Show::Add),
        r#"test_time_spring_type3_prev_type2_prev: ADD: 2010-02-11 02:18:48 EST + P+0Y1M2DT16H19M40S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_prev_type2_st: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_prev_type2_dt: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT7H38M27S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_prev_type2_post: ADD: 2010-03-13 18:38:28 EST + P+0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_st_type2_prev: ADD: 2010-03-14 00:10:20 EST + P-0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_st_type2_st: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT0H5M15S = **2010-03-14 00:15:35 EST**
test_time_spring_type3_st_type2_dt: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT2H6M35S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_st_type2_post: ADD: 2010-03-14 00:10:20 EST + P+0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EDT**
test_time_spring_type3_dt_type2_prev: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT7H38M27S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_dt_type2_st: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT2H6M35S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_dt_type2_dt: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M0DT2H3M1S = **2010-03-14 05:19:56 EDT**
test_time_spring_type3_dt_type2_post: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_post_type2_prev: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_post_type2_st: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT18H49M39S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_post_type2_dt: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_post_type2_post: ADD: 2010-03-15 18:57:55 EDT + P+0Y0M0DT1H2M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_stsec_type2_dtsec: ADD: 2010-03-14 01:59:59 EST + P+0Y0M0DT0H0M1S = **2010-03-14 03:00:00 EDT**
test_time_spring_type3_dtsec_type2_stsec: ADD: 2010-03-14 03:00:00 EDT + P-0Y0M0DT0H0M1S = **2010-03-14 01:59:59 EST**"#,
    );
}

/// ext/date/tests/DateTime_compare.phpt (partial: only the comparison of two
/// initialised objects)
#[test]
fn datetime_compare() {
    let dt = DT::new("2023-01-16 17:09:08", "UTC");
    let dti = DT::new("2023-01-16 17:09:08", "UTC");
    assert_eq!(
        vd_bool(dt.t.compare(&dti.t) == Ordering::Less),
        "bool(false)\n"
    );
}

/// ext/date/tests/DateTime_createFromImmutable-001.phpt (partial: the TypeError is
/// not ported)
#[test]
fn datetime_createfromimmutable_001() {
    let current = "2014-03-02 16:24:08";
    let i = DT::new(current, "Europe/London");
    let mut m = i.clone();
    let mut out = m.var_dump("DateTime", 1);
    assert!(m.modify("+ 1 hour"));
    out += &vd_bool(i.format("Y-m-d H:i:s") == current);
    assert_eq!(m.format("Y-m-d H:i:s"), "2014-03-02 17:24:08");
    assert_text(
        &out,
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
bool(true)"#,
    );
}

/// ext/date/tests/DateTime_days-fall-type2-type2.phpt
#[test]
fn datetime_days_fall_type2_type2() {
    assert_text(
        &examine_diff(DATA_FALL_TYPE2_TYPE2, Show::Days),
        r#"test_time_fall_type2_prev_type2_prev: DAYS: **33**
test_time_fall_type2_prev_type2_dt: DAYS: **0**
test_time_fall_type2_prev_type2_redodt: DAYS: **0**
test_time_fall_type2_prev_type2_redost: DAYS: **0**
test_time_fall_type2_prev_type2_st: DAYS: **0**
test_time_fall_type2_prev_type2_post: DAYS: **2**
test_time_fall_type2_dt_type2_prev: DAYS: **0**
test_time_fall_type2_dt_type2_dt: DAYS: **0**
test_time_fall_type2_dt_type2_redodt: DAYS: **0**
test_time_fall_type2_dt_type2_redost: DAYS: **0**
test_time_fall_type2_dt_type2_st: DAYS: **0**
test_time_fall_type2_dt_type2_post: DAYS: **1**
test_time_fall_type2_redodt_type2_prev: DAYS: **0**
test_time_fall_type2_redodt_type2_dt: DAYS: **0**
test_time_fall_type2_redodt_type2_redodt: DAYS: **0**
test_time_fall_type2_redodt_type2_redost: DAYS: **0**
test_time_fall_type2_redodt_type2_st: DAYS: **0**
test_time_fall_type2_redodt_type2_post: DAYS: **1**
test_time_fall_type2_redost_type2_prev: DAYS: **0**
test_time_fall_type2_redost_type2_dt: DAYS: **0**
test_time_fall_type2_redost_type2_redodt: DAYS: **0**
test_time_fall_type2_redost_type2_redost: DAYS: **0**
test_time_fall_type2_redost_type2_st: DAYS: **0**
test_time_fall_type2_redost_type2_post: DAYS: **1**
test_time_fall_type2_st_type2_prev: DAYS: **0**
test_time_fall_type2_st_type2_dt: DAYS: **0**
test_time_fall_type2_st_type2_redodt: DAYS: **0**
test_time_fall_type2_st_type2_redost: DAYS: **0**
test_time_fall_type2_st_type2_st: DAYS: **0**
test_time_fall_type2_st_type2_post: DAYS: **1**
test_time_fall_type2_post_type2_prev: DAYS: **2**
test_time_fall_type2_post_type2_dt: DAYS: **1**
test_time_fall_type2_post_type2_redodt: DAYS: **1**
test_time_fall_type2_post_type2_redost: DAYS: **1**
test_time_fall_type2_post_type2_st: DAYS: **1**
test_time_fall_type2_post_type2_post: DAYS: **0**
test_time_fall_type2_dtsec_type2_stsec: DAYS: **0**
test_time_fall_type2_stsec_type2_dtsec: DAYS: **0**"#,
    );
}

/// ext/date/tests/DateTime_days-spring-type2-type2.phpt
#[test]
fn datetime_days_spring_type2_type2() {
    assert_text(
        &examine_diff(DATA_SPRING_TYPE2_TYPE2, Show::Days),
        r#"test_time_spring_type2_prev_type2_prev: DAYS: **30**
test_time_spring_type2_prev_type2_st: DAYS: **0**
test_time_spring_type2_prev_type2_dt: DAYS: **0**
test_time_spring_type2_prev_type2_post: DAYS: **2**
test_time_spring_type2_st_type2_prev: DAYS: **0**
test_time_spring_type2_st_type2_st: DAYS: **0**
test_time_spring_type2_st_type2_dt: DAYS: **0**
test_time_spring_type2_st_type2_post: DAYS: **1**
test_time_spring_type2_dt_type2_prev: DAYS: **0**
test_time_spring_type2_dt_type2_st: DAYS: **0**
test_time_spring_type2_dt_type2_dt: DAYS: **0**
test_time_spring_type2_dt_type2_post: DAYS: **1**
test_time_spring_type2_post_type2_prev: DAYS: **2**
test_time_spring_type2_post_type2_st: DAYS: **1**
test_time_spring_type2_post_type2_dt: DAYS: **1**
test_time_spring_type2_post_type2_post: DAYS: **0**
test_time_spring_type2_stsec_type2_dtsec: DAYS: **0**
test_time_spring_type2_dtsec_type2_stsec: DAYS: **0**"#,
    );
}

/// ext/date/tests/DateTime_diff-fall-type2-type2.phpt
#[test]
fn datetime_diff_fall_type2_type2() {
    assert_text(
        &examine_diff(DATA_FALL_TYPE2_TYPE2, Show::Diff),
        r#"test_time_fall_type2_prev_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-10-04 02:18:48 EDT = **P+0Y1M2DT16H19M40S**
test_time_fall_type2_prev_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT5H31M52S**
test_time_fall_type2_prev_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT6H34M5S**
test_time_fall_type2_prev_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT7H36M16S**
test_time_fall_type2_prev_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT9H38M27S**
test_time_fall_type2_prev_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M2DT2H21M31S**
test_time_fall_type2_dt_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 00:10:20 EDT = **P-0Y0M0DT5H31M52S**
test_time_fall_type2_dt_type2_dt: DIFF: 2010-11-07 00:15:35 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT0H5M15S**
test_time_fall_type2_dt_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT1H2M13S**
test_time_fall_type2_dt_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT2H4M24S**
test_time_fall_type2_dt_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT4H6M35S**
test_time_fall_type2_dt_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M1DT20H49M39S**
test_time_fall_type2_redodt_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT6H34M5S**
test_time_fall_type2_redodt_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT1H2M13S**
test_time_fall_type2_redodt_type2_redodt: DIFF: 2010-11-07 01:15:35 EDT - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT0H3M2S**
test_time_fall_type2_redodt_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT1H2M11S**
test_time_fall_type2_redodt_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT3H4M22S**
test_time_fall_type2_redodt_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M1DT19H47M26S**
test_time_fall_type2_redost_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT7H36M16S**
test_time_fall_type2_redost_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT2H4M24S**
test_time_fall_type2_redost_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT1H2M11S**
test_time_fall_type2_redost_type2_redost: DIFF: 2010-11-07 01:16:54 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT0H2M10S**
test_time_fall_type2_redost_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT2H2M11S**
test_time_fall_type2_redost_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:14:44 EST = **P+0Y0M1DT18H45M15S**
test_time_fall_type2_st_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT9H38M27S**
test_time_fall_type2_st_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT4H6M35S**
test_time_fall_type2_st_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT3H4M22S**
test_time_fall_type2_st_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 03:16:55 EST = **P-0Y0M0DT2H2M11S**
test_time_fall_type2_st_type2_st: DIFF: 2010-11-07 05:19:56 EST - 2010-11-07 03:16:55 EST = **P+0Y0M0DT2H3M1S**
test_time_fall_type2_st_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 03:16:55 EST = **P+0Y0M1DT16H43M4S**
test_time_fall_type2_post_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M2DT2H21M31S**
test_time_fall_type2_post_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT20H49M39S**
test_time_fall_type2_post_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT19H47M26S**
test_time_fall_type2_post_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT18H45M15S**
test_time_fall_type2_post_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT16H43M4S**
test_time_fall_type2_post_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-08 18:57:55 EST = **P+0Y0M0DT1H2M4S**
test_time_fall_type2_dtsec_type2_stsec: DIFF: 2010-11-07 01:00:00 EST - 2010-11-07 01:59:59 EDT = **P+0Y0M0DT0H0M1S**
test_time_fall_type2_stsec_type2_dtsec: DIFF: 2010-11-07 01:59:59 EDT - 2010-11-07 01:00:00 EST = **P-0Y0M0DT0H0M1S**"#,
    );
}

/// ext/date/tests/DateTime_diff-spring-type2-type2.phpt
#[test]
fn datetime_diff_spring_type2_type2() {
    assert_text(
        &examine_diff(DATA_SPRING_TYPE2_TYPE2, Show::Diff),
        r#"test_time_spring_type2_prev_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-02-11 02:18:48 EST = **P+0Y1M2DT16H19M40S**
test_time_spring_type2_prev_type2_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-13 18:38:28 EST = **P+0Y0M0DT5H31M52S**
test_time_spring_type2_prev_type2_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M0DT7H38M27S**
test_time_spring_type2_prev_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M2DT0H21M31S**
test_time_spring_type2_st_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 00:10:20 EST = **P-0Y0M0DT5H31M52S**
test_time_spring_type2_st_type2_st: DIFF: 2010-03-14 00:15:35 EST - 2010-03-14 00:10:20 EST = **P+0Y0M0DT0H5M15S**
test_time_spring_type2_st_type2_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M0DT2H6M35S**
test_time_spring_type2_st_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M1DT18H49M39S**
test_time_spring_type2_dt_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT7H38M27S**
test_time_spring_type2_dt_type2_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT2H6M35S**
test_time_spring_type2_dt_type2_dt: DIFF: 2010-03-14 05:19:56 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M0DT2H3M1S**
test_time_spring_type2_dt_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M1DT16H43M4S**
test_time_spring_type2_post_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M2DT0H21M31S**
test_time_spring_type2_post_type2_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT18H49M39S**
test_time_spring_type2_post_type2_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT16H43M4S**
test_time_spring_type2_post_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-15 18:57:55 EDT = **P+0Y0M0DT1H2M4S**
test_time_spring_type2_stsec_type2_dtsec: DIFF: 2010-03-14 03:00:00 EDT - 2010-03-14 01:59:59 EST = **P+0Y0M0DT0H0M1S**
test_time_spring_type2_dtsec_type2_stsec: DIFF: 2010-03-14 01:59:59 EST - 2010-03-14 03:00:00 EDT = **P-0Y0M0DT0H0M1S**"#,
    );
}

/// ext/date/tests/DateTime_extends_basic3.phpt (partial: the overriding method's
/// default format)
#[test]
fn datetime_extends_basic3() {
    let d = DT::new("1967-05-01 22:30:41", "Europe/London");
    assert_eq!(d.format("F j, Y, g:i:s a"), "May 1, 1967, 10:30:41 pm");
}

/// ext/date/tests/DateTime_setISODate_basic1.phpt
#[test]
fn datetime_setisodate_basic1() {
    let mut out = String::from("*** Testing DateTime::setISODate() : basic functionality ***\n");
    let mut datetime = DT::new("2009-01-30 17:57:32", "Europe/London");
    datetime.set_isodate(2008, 40, 1);
    let _ = writeln!(out, "Week 40 of 2009 is in \"{}\"", datetime.format("F"));
    datetime.set_isodate(2009, 30, 3);
    let _ = writeln!(
        out,
        "Week 30 day 3 of 2009 is \"{}\"",
        datetime.format("D M j")
    );
    datetime.set_isodate(2008, 30, 3);
    let _ = writeln!(
        out,
        "..same day last year was \"{}\"",
        datetime.format("D M j")
    );
    assert_text(
        &out,
        r#"*** Testing DateTime::setISODate() : basic functionality ***
Week 40 of 2009 is in "September"
Week 30 day 3 of 2009 is "Wed Jul 22"
..same day last year was "Wed Jul 23""#,
    );
}

/// ext/date/tests/DateTime_sub-fall-type2-type2.phpt
#[test]
fn datetime_sub_fall_type2_type2() {
    assert_text(
        &examine_diff(DATA_FALL_TYPE2_TYPE2, Show::Sub),
        r#"test_time_fall_type2_prev_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P+0Y1M2DT16H19M40S = **2010-10-04 02:18:48 EDT**
test_time_fall_type2_prev_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P+0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type2_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT7H36M16S = **2010-11-06 17:38:28 EST**
test_time_fall_type2_prev_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT9H38M27S = **2010-11-06 17:38:28 EST**
test_time_fall_type2_prev_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EST**
test_time_fall_type2_dt_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type2_dt: SUB: 2010-11-07 00:15:35 EDT - P+0Y0M0DT0H5M15S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type2_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT2H4M24S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_dt_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT4H6M35S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_dt_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT20H49M39S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_redodt_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type2_redodt: SUB: 2010-11-07 01:15:35 EDT - P+0Y0M0DT0H3M2S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type2_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT1H2M11S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_redodt_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT3H4M22S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_redodt_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT19H47M26S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_redost_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT7H36M16S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_redost_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT2H4M24S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_redost_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT1H2M11S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_redost_type2_redost: SUB: 2010-11-07 01:16:54 EST - P+0Y0M0DT0H2M10S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_redost_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_redost_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_st_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT9H38M27S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_st_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT4H6M35S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_st_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT3H4M22S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_st_type2_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_st_type2_st: SUB: 2010-11-07 05:19:56 EST - P+0Y0M0DT2H3M1S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_st_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_post_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EDT**
test_time_fall_type2_post_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EDT**
test_time_fall_type2_post_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EDT**
test_time_fall_type2_post_type2_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_post_type2_st: SUB: 2010-11-07 03:16:55 EST - P-0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_post_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M0DT1H2M4S = **2010-11-08 18:57:55 EST**
test_time_fall_type2_dtsec_type2_stsec: SUB: 2010-11-07 01:00:00 EST - P+0Y0M0DT0H0M1S = **2010-11-07 00:59:59 EST**
test_time_fall_type2_stsec_type2_dtsec: SUB: 2010-11-07 01:59:59 EDT - P-0Y0M0DT0H0M1S = **2010-11-07 02:00:00 EDT**"#,
    );
}

/// ext/date/tests/DateTime_sub-spring-type2-type2.phpt
#[test]
fn datetime_sub_spring_type2_type2() {
    assert_text(
        &examine_diff(DATA_SPRING_TYPE2_TYPE2, Show::Sub),
        r#"test_time_spring_type2_prev_type2_prev: SUB: 2010-03-13 18:38:28 EST - P+0Y1M2DT16H19M40S = **2010-02-11 02:18:48 EST**
test_time_spring_type2_prev_type2_st: SUB: 2010-03-14 00:10:20 EST - P+0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_prev_type2_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT7H38M27S = **2010-03-13 19:38:28 EDT**
test_time_spring_type2_prev_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EDT**
test_time_spring_type2_st_type2_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_st_type2_st: SUB: 2010-03-14 00:15:35 EST - P+0Y0M0DT0H5M15S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_st_type2_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT2H6M35S = **2010-03-14 01:10:20 EDT**
test_time_spring_type2_st_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT18H49M39S = **2010-03-14 01:10:20 EDT**
test_time_spring_type2_dt_type2_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT7H38M27S = **2010-03-14 02:16:55 EST**
test_time_spring_type2_dt_type2_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M0DT2H6M35S = **2010-03-14 02:16:55 EST**
test_time_spring_type2_dt_type2_dt: SUB: 2010-03-14 05:19:56 EDT - P+0Y0M0DT2H3M1S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_dt_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_post_type2_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EST**
test_time_spring_type2_post_type2_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EST**
test_time_spring_type2_post_type2_dt: SUB: 2010-03-14 03:16:55 EDT - P-0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type2_post_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M0DT1H2M4S = **2010-03-15 18:57:55 EDT**
test_time_spring_type2_stsec_type2_dtsec: SUB: 2010-03-14 03:00:00 EDT - P+0Y0M0DT0H0M1S = **2010-03-14 02:59:59 EDT**
test_time_spring_type2_dtsec_type2_stsec: SUB: 2010-03-14 01:59:59 EST - P-0Y0M0DT0H0M1S = **2010-03-14 02:00:00 EST**"#,
    );
}

/// ext/date/tests/bug-gh11600.phpt (partial: the IntlDateFormatter output is
/// supplied literally, with both a plain space and ICU's narrow no-break space)
#[test]
fn bug_gh11600() {
    assert_eq!(st("2023-07-11 16:02", "UTC"), 1689091320);
    for time_string in ["4:02 PM", "4:02\u{202f}PM"] {
        assert_eq!(
            strtotime_abs(&format!("2023-07-11 {time_string}"), "UTC"),
            Some(1689091320),
            "{time_string:?}"
        );
    }
}

/// ext/date/tests/bug-gh8964-001.phpt
#[test]
fn bug_gh8964_001() {
    let actual = DT::new("2022-07-21 15:00:10", "UTC");
    let mut delta = php::date_interval_create("PT0S").unwrap();
    delta.diff.us = (0.9f64 * 1_000_000.0) as i64;
    let mut expected_lower = actual.clone();
    expected_lower.sub_interval(&delta);
    let mut expected_upper = actual.clone();
    expected_upper.add_interval(&delta);
    let mut out = String::new();
    for d in [&expected_lower, &actual, &expected_upper] {
        out += &d.format("H:i:s.u U");
        out.push('\n');
    }
    out += &vd_bool(actual.t.compare(&expected_lower.t) == Ordering::Less);
    out += &vd_bool(actual.t.compare(&expected_upper.t) == Ordering::Greater);
    assert_text(
        &out,
        r#"15:00:09.100000 1658415609
15:00:10.000000 1658415610
15:00:10.900000 1658415610
bool(false)
bool(false)"#,
    );
}

/// ext/date/tests/bug17988.phpt
#[test]
fn bug17988() {
    let mut out = String::new();
    for s in [
        "2002-06-25 14:18:48.543728",
        "2002-06-25 14:18:48.543728 GMT",
        "2002-06-25 14:18:48.543728 EDT",
        "2002-06-25 14:18:48.543728-00",
        "2002-06-25 14:18:48.543728+00",
        "2002-06-25 14:18:48.543728-04",
        "2002-06-25 14:18:48.543728+04",
        "2002-06-25 14:18:48.543728-0300",
        "2002-06-25 14:18:48.543728+0300",
        "2002-06-25 14:18:48.543728-0330",
        "2002-06-25 14:18:48.543728+0330",
    ] {
        out += &gmdate("Y-m-d H:i:s", st(s, "GMT"));
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2002-06-25 14:18:48
2002-06-25 14:18:48
2002-06-25 18:18:48
2002-06-25 14:18:48
2002-06-25 14:18:48
2002-06-25 18:18:48
2002-06-25 10:18:48
2002-06-25 17:18:48
2002-06-25 11:18:48
2002-06-25 17:48:48
2002-06-25 10:48:48"#,
    );
}

/// ext/date/tests/bug26198.phpt
#[test]
fn bug26198() {
    let mut out = gmdate("F Y (Y-m-d H:i:s T)\n", st("Oct 2001", "GMT"));
    out += &gmdate("M Y (Y-m-d H:i:s T)\n", st("2001 Oct", "GMT"));
    assert_text(
        &out,
        r#"October 2001 (2001-10-01 00:00:00 GMT)
Oct 2001 (2001-10-01 00:00:00 GMT)"#,
    );
}

/// ext/date/tests/bug28088.phpt
#[test]
fn bug28088() {
    assert_eq!(
        gmdate("m/d/y Hi", st("04/04/04 2345", "UTC")),
        "04/04/04 2345"
    );
    assert_eq!(
        gmdate("m/d/y Hi", st("04/04/04 0045", "UTC")),
        "04/04/04 0045"
    );
}

/// ext/date/tests/bug30532.phpt
#[test]
fn bug30532() {
    let zone = "America/New_York";
    let mut out = String::new();
    for (i, s) in [
        "2004-10-31 EDT +1 hour",
        "2004-10-31 EDT +2 hours",
        "2004-10-31 EDT +3 hours",
        "2004-10-31 +1 hour",
        "2004-10-31 +2 hours",
        "2004-10-31 +3 hours",
    ]
    .into_iter()
    .enumerate()
    {
        if i == 3 {
            out.push('\n');
        }
        out += &date("Y-m-d H:i:s T", st(s, zone), zone);
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2004-10-31 01:00:00 EDT
2004-10-31 01:00:00 EST
2004-10-31 02:00:00 EST

2004-10-31 01:00:00 EDT
2004-10-31 02:00:00 EST
2004-10-31 03:00:00 EST"#,
    );
}

/// ext/date/tests/bug33414-1.phpt (the fixed "TZ=" / "wanted=" lines are not
/// compared)
#[test]
fn bug33414_1() {
    let cases = [
        ("America/Mendoza", 8327, "next Sunday"),
        ("America/Catamarca", 7599, "next Sunday"),
        ("America/Cordoba", 7599, "next Sunday"),
        ("America/Rosario", 7958, "next Tuesday"),
        ("Europe/Vienna", 3746, "next Thursday"),
        ("Asia/Baku", 8299, "second Monday"),
        ("America/Noronha", 10866, "next Friday"),
        ("America/Havana", 12720, "next Thursday"),
        ("Europe/Tallinn", 11777, "next Saturday"),
        ("Asia/Jerusalem", 13056, "next Thursday"),
        ("Europe/Vilnius", 12140, "next Friday"),
        ("Pacific/Kwajalein", 8627, "next Saturday"),
        ("Asia/Ulan_Bator", 11588, "next Saturday"),
        ("America/Cancun", 11785, "next Sunday"),
        ("America/Mexico_City", 11781, "next Wednesday"),
        ("America/Mazatlan", 11780, "next Tuesday"),
        ("America/Chihuahua", 11782, "next Thursday"),
        ("Asia/Kuala_Lumpur", 4380, "next Monday"),
        ("Pacific/Chatham", 1762, "next Monday"),
        ("America/Lima", 5839, "next Thursday"),
        ("Asia/Karachi", 11783, "next Friday"),
        ("America/Asuncion", 11746, "next Wednesday"),
        ("Asia/Singapore", 4383, "next Thursday"),
        ("America/Montevideo", 12678, "next Thursday"),
    ];
    let mut out = String::new();
    for (zone, day, rel) in cases {
        let t_stamp = mktime(17, 17, 17, 1, day, 1970, zone);
        let _ = writeln!(out, "tStamp={}", date("l Y-m-d H:i:s T I", t_stamp, zone));
        let r = strtotime(rel, t_stamp, zone).unwrap();
        let _ = writeln!(out, "result={}", date("l Y-m-d H:i:s T I", r, zone));
    }
    let expected: Vec<&str> = r#"TZ=America/Mendoza - wrong day.
tStamp=Sunday 1992-10-18 17:17:17 -02 1
result=Sunday 1992-10-25 00:00:00 -02 1
wanted=Sunday              00:00:00

TZ=America/Catamarca - wrong day.
tStamp=Sunday 1990-10-21 17:17:17 -02 1
result=Sunday 1990-10-28 00:00:00 -02 1
wanted=Sunday              00:00:00

TZ=America/Cordoba - wrong day.
tStamp=Sunday 1990-10-21 17:17:17 -02 1
result=Sunday 1990-10-28 00:00:00 -02 1
wanted=Sunday              00:00:00

TZ=America/Rosario - wrong day.
tStamp=Tuesday 1991-10-15 17:17:17 -04 0
result=Tuesday 1991-10-22 00:00:00 -02 1
wanted=Tuesday           00:00:00

TZ=Europe/Vienna - wrong day - giving unexpected results, at
least on my system :-)
tStamp=Thursday 1980-04-03 17:17:17 CET 0
result=Thursday 1980-04-10 00:00:00 CEST 1
wanted=Thursday             00:00:00

TZ=Asia/Baku - wrong day.
tStamp=Sunday 1992-09-20 17:17:17 +04 1
result=Monday 1992-09-28 00:00:00 +04 0
wanted=Monday              00:00:00

TZ=America/Noronha - wrong day.
tStamp=Friday 1999-10-01 17:17:17 -02 0
result=Friday 1999-10-08 00:00:00 -01 1
wanted=Friday              00:00:00

TZ=America/Havana - wrong day.
tStamp=Thursday 2004-10-28 17:17:17 CDT 1
result=Thursday 2004-11-04 00:00:00 CDT 1
wanted=Thursday             00:00:00

TZ=Europe/Tallinn - wrong day.
tStamp=Saturday 2002-03-30 17:17:17 EET 0
result=Saturday 2002-04-06 00:00:00 EEST 1
wanted=Saturday          00:00:00

TZ=Asia/Jerusalem - wrong day.
tStamp=Thursday 2005-09-29 17:17:17 IDT 1
result=Thursday 2005-10-06 00:00:00 IDT 1
wanted=Thursday             00:00:00

TZ=Europe/Vilnius - wrong day.
tStamp=Friday 2003-03-28 17:17:17 EET 0
result=Friday 2003-04-04 00:00:00 EEST 1
wanted=Friday            00:00:00

TZ=Pacific/Kwajalein - wrong day.
tStamp=Saturday 1993-08-14 17:17:17 -12 0
result=Sunday 1993-08-22 00:00:00 +12 0
wanted=Sunday            00:00:00

TZ=Asia/Ulan_Bator - wrong day.
tStamp=Saturday 2001-09-22 17:17:17 +09 1
result=Saturday 2001-09-29 00:00:00 +09 1
wanted=Saturday            00:00:00

TZ=America/Cancun - wrong day.
tStamp=Sunday 2002-04-07 17:17:17 CDT 1
result=Sunday 2002-04-14 00:00:00 CDT 1
wanted=Sunday            00:00:00

TZ=America/Mexico_City - wrong day.
tStamp=Wednesday 2002-04-03 17:17:17 CST 0
result=Wednesday 2002-04-10 00:00:00 CDT 1
wanted=Wednesday            00:00:00

TZ=America/Mazatlan - wrong day.
tStamp=Tuesday 2002-04-02 17:17:17 MST 0
result=Tuesday 2002-04-09 00:00:00 MDT 1
wanted=Tuesday            00:00:00

TZ=America/Chihuahua - wrong day.
tStamp=Thursday 2002-04-04 17:17:17 MST 0
result=Thursday 2002-04-11 00:00:00 MDT 1
wanted=Thursday            00:00:00

TZ=Asia/Kuala_Lumpur - wrong day.
tStamp=Monday 1981-12-28 17:17:17 +0730 0
result=Monday 1982-01-04 00:00:00 +08 0
wanted=Monday            00:00:00

TZ=Pacific/Chatham - wrong day.
tStamp=Monday 1974-10-28 17:17:17 +1245 0
result=Monday 1974-11-04 00:00:00 +1345 1
wanted=Monday            00:00:00

TZ=America/Lima - wrong day.
tStamp=Thursday 1985-12-26 17:17:17 -05 0
result=Thursday 1986-01-02 00:00:00 -04 1
wanted=Thursday            00:00:00

TZ=Asia/Karachi - wrong day.
tStamp=Friday 2002-04-05 17:17:17 PKT 0
result=Friday 2002-04-12 00:00:00 PKST 1
wanted=Friday            00:00:00

TZ=America/Asuncion - wrong day.
tStamp=Wednesday 2002-02-27 17:17:17 -03 1
result=Wednesday 2002-03-06 00:00:00 -03 1
wanted=Wednesday            00:00:00

TZ=Asia/Singapore - wrong day.
tStamp=Thursday 1981-12-31 17:17:17 +0730 0
result=Thursday 1982-01-07 00:00:00 +08 0
wanted=Thursday             00:00:00

TZ=America/Montevideo - wrong day.
tStamp=Thursday 2004-09-16 17:17:17 -03 0
result=Thursday 2004-09-23 00:00:00 -02 1
wanted=Thursday             00:00:00"#
        .lines()
        .filter(|l| l.starts_with("tStamp=") || l.starts_with("result="))
        .collect();
    assert_text(&out, &expected.join("\n"));
}

/// ext/date/tests/bug33536.phpt
#[test]
fn bug33536() {
    assert_eq!(strtotime_abs("monkey", "GMT"), None);
}

/// ext/date/tests/bug34087.phpt
#[test]
fn bug34087() {
    let mut out = format!("Y/m/d: {}\n", st("2005/8/12", "UTC"));
    let _ = writeln!(out, "Y-m-d: {}", st("2005-8-12", "UTC"));
    for s in ["2005/1/2", "2005/01/02", "2005/01/2", "2005/1/02"] {
        out += &date(ISO8601, st(s, "UTC"), "UTC");
        out.push('\n');
    }
    assert_text(
        &out,
        r#"Y/m/d: 1123804800
Y-m-d: 1123804800
2005-01-02T00:00:00+0000
2005-01-02T00:00:00+0000
2005-01-02T00:00:00+0000
2005-01-02T00:00:00+0000"#,
    );
}

/// ext/date/tests/bug35414.phpt
#[test]
fn bug35414() {
    let mut out = String::new();
    let ts = [
        st("Sat 26th Nov 2005 18:18", "UTC"),
        strtotime("26th Nov", 1134340285, "UTC").unwrap(),
        st("Dec. 4th, 2005", "UTC"),
        st("December 4th, 2005", "UTC"),
    ];
    for t in ts {
        out += &date(ISO8601, t, "UTC");
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2005-11-26T18:18:00+0000
2005-11-26T00:00:00+0000
2005-12-04T00:00:00+0000
2005-12-04T00:00:00+0000"#,
    );
}

/// ext/date/tests/bug35630.phpt
#[test]
fn bug35630() {
    assert_eq!(
        date(ISO8601, st("5 january 2006+3day+1day", "UTC"), "UTC"),
        "2006-01-09T00:00:00+0000"
    );
}

/// ext/date/tests/bug36510.phpt
#[test]
fn bug36510() {
    let t = 1140973388;
    assert_eq!(strtotime("-2 hours", t, "UTC"), Some(1140966188));
    assert_eq!(strtotime("-2\thours", t, "UTC"), Some(1140966188));
}

/// ext/date/tests/bug37616.phpt
#[test]
fn bug37616() {
    let zone = "Europe/Oslo";
    assert_eq!(
        vd_str(&date(
            "D, d M y H:i:s O",
            st("1 Jul 06 14:27:30 +0200", zone),
            zone
        )),
        "string(29) \"Sat, 01 Jul 06 14:27:30 +0200\"\n"
    );
}

/// ext/date/tests/bug41523-64bit.phpt
#[test]
fn bug41523_64bit() {
    let s = "0000-00-00 00:00:00";
    let mut out = date_parse(s).var_dump();
    out += &V::Int(st(s, "UTC")).var_dump();
    let dt = DT::new(s, "UTC");
    out += &dt.var_dump("DateTime", 1);
    out += &dt.format(ISO8601);
    out.push('\n');
    assert_text(
        &out,
        r#"array(12) {
  ["year"]=>
  int(0)
  ["month"]=>
  int(0)
  ["day"]=>
  int(0)
  ["hour"]=>
  int(0)
  ["minute"]=>
  int(0)
  ["second"]=>
  int(0)
  ["fraction"]=>
  float(0)
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [20]=>
    string(27) "The parsed date was invalid"
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
int(-62169984000)
object(DateTime)#1 (3) {
  ["date"]=>
  string(27) "-0001-11-30 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
-0001-11-30T00:00:00+0000"#,
    );
}

/// ext/date/tests/bug41964.phpt
#[test]
fn bug41964() {
    let mut out = String::new();
    for s in [
        "Ask the Experts",
        "A ",
        "A",
        "a ",
        "a",
        "A Revolution in Development",
        "a nothing",
    ] {
        let res = date_parse(s);
        out += &vd_opt(get(&res, "zone"));
        out += &vd_opt(get(&res, "tz_abbr"));
        out.push('\n');
    }
    assert_text(
        &out,
        r#"NULL
NULL

int(3600)
string(1) "A"

int(3600)
string(1) "A"

int(3600)
string(1) "A"

int(3600)
string(1) "A"

int(3600)
string(1) "A"

int(3600)
string(1) "A""#,
    );
}

/// ext/date/tests/bug43808.phpt
#[test]
fn bug43808() {
    let mut out = match DT::try_new("asdfasdf", None, "UTC") {
        Ok(d) => d.var_dump("DateTime", 1),
        Err(_) => vd_bool(false),
    };
    let errors = last_errors(None, "asdfasdf").expect("errors").var_dump();
    out += &errors;
    out += &errors;
    assert_text(
        &out,
        r#"bool(false)
array(4) {
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [6]=>
    string(29) "Double timezone specification"
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [0]=>
    string(47) "The timezone could not be found in the database"
  }
}
array(4) {
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [6]=>
    string(29) "Double timezone specification"
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [0]=>
    string(47) "The timezone could not be found in the database"
  }
}"#,
    );
}

/// ext/date/tests/bug45529.phpt
#[test]
fn bug45529() {
    let zone = "Europe/Oslo";
    let tz1 = php::timezone_open("UTC").unwrap();
    let tz2 = PhpTimeZone::from_time(&DT::new("UTC", zone).t).unwrap();
    let mut out = format!("{}\n{}\n", tz1.name(), tz2.name());
    for z in [&tz1, &tz2] {
        let mut d = DT::new("2008-01-01 12:00:00+0200", zone);
        php::date_timezone_set(&mut d.t, z);
        out += &d.format(ISO8601);
        out.push('\n');
    }
    assert_text(
        &out,
        r#"UTC
UTC
2008-01-01T10:00:00+0000
2008-01-01T10:00:00+0000"#,
    );
}

/// ext/date/tests/bug46111.phpt
#[test]
fn bug46111() {
    let zones = identifiers_list();
    assert!(zones.len() > 400);
    let failed: Vec<&String> = zones
        .iter()
        .filter(|z| strtotime_abs(&format!("2008-01-01 13:00:00 {z}"), "Asia/Calcutta").is_none())
        .collect();
    assert!(
        failed.is_empty(),
        "strtotime(timezone) == false: {failed:?}"
    );
}

/// ext/date/tests/bug49778.phpt
#[test]
fn bug49778() {
    let i = php::date_interval_create("P7D").unwrap();
    let mut out = interval_var_dump(&i, 1);
    out += &interval_format(&i, "%d");
    out.push('\n');
    out += &interval_format(&i, "%a");
    out.push('\n');
    assert_text(
        &out,
        &r#"object(DateInterval)#%d (%d) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(7)
  ["h"]=>
  int(0)
  ["i"]=>
  int(0)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  bool(false)
  ["from_string"]=>
  bool(false)
}
7
(unknown)"#
            .replace("#%d (%d)", "#1 (10)"),
    );
}

/// ext/date/tests/bug51096.phpt
#[test]
fn bug51096() {
    let echo = |v: Option<&V>| v.map_or_else(String::new, V::print_r);
    let mut out = String::new();
    for test in [
        "first day",
        "last day",
        "next month",
        "first day next month",
        "last day next month",
        "first day of next month",
        "last day of next month",
    ] {
        let result = date_parse(test);
        let rel = get(&result, "relative").expect("relative");
        let _ = writeln!(
            out,
            "{test}\n- month: {}; day: {}; first-day-of: {}; last-day-of: {}",
            echo(get(rel, "month")),
            echo(get(rel, "day")),
            get(rel, "first_day_of_month").is_some(),
            get(rel, "last_day_of_month").is_some()
        );
        let mut d = DT::new("2010-03-06 15:21 UTC", "UTC");
        let _ = write!(out, "- {}", d.format(ISO8601));
        assert!(d.modify(test));
        let _ = writeln!(out, " -> {}\n", d.format(ISO8601));
    }
    assert_text(
        &out,
        r#"first day
- month: 0; day: 1; first-day-of: false; last-day-of: false
- 2010-03-06T15:21:00+0000 -> 2010-03-07T15:21:00+0000

last day
- month: 0; day: -1; first-day-of: false; last-day-of: false
- 2010-03-06T15:21:00+0000 -> 2010-03-05T15:21:00+0000

next month
- month: 1; day: 0; first-day-of: false; last-day-of: false
- 2010-03-06T15:21:00+0000 -> 2010-04-06T15:21:00+0000

first day next month
- month: 1; day: 1; first-day-of: false; last-day-of: false
- 2010-03-06T15:21:00+0000 -> 2010-04-07T15:21:00+0000

last day next month
- month: 1; day: -1; first-day-of: false; last-day-of: false
- 2010-03-06T15:21:00+0000 -> 2010-04-05T15:21:00+0000

first day of next month
- month: 1; day: 0; first-day-of: true; last-day-of: false
- 2010-03-06T15:21:00+0000 -> 2010-04-01T15:21:00+0000

last day of next month
- month: 1; day: 0; first-day-of: false; last-day-of: true
- 2010-03-06T15:21:00+0000 -> 2010-04-30T15:21:00+0000"#,
    );
}

/// ext/date/tests/bug51994.phpt
#[test]
fn bug51994() {
    assert_text(
        &date_parse_from_format("yz", "10153").var_dump(),
        r#"array(12) {
  ["year"]=>
  int(2010)
  ["month"]=>
  int(6)
  ["day"]=>
  int(3)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}"#,
    );
}

/// ext/date/tests/bug52342.phpt (`new DateTime()` uses the fixed common::NOW; the
/// result does not depend on it)
#[test]
fn bug52342() {
    let zone = "Europe/Berlin";
    let mut from = DT::new("now", zone);
    from.set_time(0, 0, 0, 0);
    from.set_isodate(2010, 28, 1);
    let mut out = format!("{}\n", from.format("d.m.Y H:i"));
    let _ = writeln!(out, "{}", from.timestamp());
    let _ = writeln!(out, "{}", date("d.m.Y H:i", from.timestamp(), zone));
    from.add("P0D");
    let _ = writeln!(out, "{}", from.timestamp());
    let _ = writeln!(out, "{}", date("d.m.Y H:i", from.timestamp(), zone));
    assert_text(
        &out,
        r#"12.07.2010 00:00
1278885600
12.07.2010 00:00
1278885600
12.07.2010 00:00"#,
    );
}

/// ext/date/tests/bug52738.phpt (partial: the subclass property is modelled
/// literally)
#[test]
fn bug52738() {
    let mut i = php::date_interval_create("P10D").unwrap();
    i.diff.d += 1;
    let mut props = vec![(sk("unit"), V::Int(42))];
    props.extend(interval_props(&i));
    let out = format!("1\n2\n42\n{}", V::Obj("di".into(), 1, props).print_r());
    assert_text(
        &out,
        r#"1
2
42
di Object
(
    [unit] => 42
    [y] => 0
    [m] => 0
    [d] => 11
    [h] => 0
    [i] => 0
    [s] => 0
    [f] => 0
    [invert] => 0
    [days] => 
    [from_string] => 
)"#,
    );
}

/// ext/date/tests/bug53437_var4.phpt
#[test]
fn bug53437_var4() {
    let zone = "Europe/Berlin";
    let dt = DT::new("2009-10-11", zone);
    let df = dt.diff(&DT::new("2009-10-13", zone));
    let mut out = interval_var_dump(&df, 3);
    let d = &df.diff;
    for v in [
        V::Int(d.y),
        V::Int(d.m),
        V::Int(d.d),
        V::Int(d.h),
        V::Int(d.i),
        V::Int(d.s),
        V::Float(d.us as f64 / 1_000_000.0),
        V::Int(i64::from(d.invert)),
        interval_days(d).map_or(V::Bool(false), V::Int),
    ] {
        out += &v.var_dump();
    }
    assert_text(
        &out,
        &r#"object(DateInterval)#%d (%d) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(2)
  ["h"]=>
  int(0)
  ["i"]=>
  int(0)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  int(2)
  ["from_string"]=>
  bool(false)
}
int(0)
int(0)
int(2)
int(0)
int(0)
int(0)
float(0)
int(0)
int(2)"#
            .replace("#%d (%d)", "#3 (10)"),
    );
}

/// ext/date/tests/bug54316.phpt
#[test]
fn bug54316() {
    let mut out = String::new();
    for (id, f) in [(1, "Y-m-d|"), (2, "Y-m-d!")] {
        out += &DT::from_format(f, "2011-02-02", None, "UTC")
            .expect("createFromFormat")
            .var_dump("DateTime", id);
    }
    assert_text(
        &out,
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2011-02-02 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#,
    );
}

/// ext/date/tests/bug62326.phpt
#[test]
fn bug62326() {
    let zone = "Europe/Berlin";
    let start = DT::new("2012-06-01", zone);
    let end = DT::new("2012-12-01", zone);
    let difference = start.diff(&end);
    assert_eq!(
        interval_format(&difference, "%mM / %dD %hH %iM"),
        "6M / 0D 0H 0M"
    );
}

/// ext/date/tests/bug62896.phpt
#[test]
fn bug62896() {
    let zone = "Europe/Berlin";
    let show = |d: &DT| format!("{} (offset {})\n", d.format("Y-m-d H:i:s T"), d.offset());
    let ts = st("2012-08-22 00:00:00 CEST", zone);
    let mut out = String::from("FROM TIMESTAMP, NO TZ:\n");
    let mut date = DT::new(&format!("@{ts}"), zone);
    out += &show(&date);
    assert!(date.modify("+0 days"));
    out += &show(&date);
    out += "FROM TIMESTAMP, WITH TZ:\n";
    let mut date = DT::new(&format!("@{ts}"), zone);
    date.set_timezone(zone);
    out += &show(&date);
    assert!(date.modify("+0 days"));
    out += &show(&date);
    out += "FROM STRING:\n";
    let mut date = DT::new_in("2012-08-22 00:00:00 CEST", zone, zone);
    out += &show(&date);
    assert!(date.modify("+0 days"));
    out += &show(&date);
    assert_text(
        &out,
        r#"FROM TIMESTAMP, NO TZ:
2012-08-21 22:00:00 GMT+0000 (offset 0)
2012-08-21 22:00:00 GMT+0000 (offset 0)
FROM TIMESTAMP, WITH TZ:
2012-08-22 00:00:00 CEST (offset 7200)
2012-08-22 00:00:00 CEST (offset 7200)
FROM STRING:
2012-08-22 00:00:00 CEST (offset 7200)
2012-08-22 00:00:00 CEST (offset 7200)"#,
    );
}

/// ext/date/tests/bug64887.phpt
#[test]
fn bug64887() {
    let datetime = DT::new("2016-10-07 13:25:50", "UTC");
    let mut out = String::new();
    for test in [
        "+1 ms",
        "-2 msec",
        "+3 msecs",
        "-4 millisecond",
        "+5 milliseconds",
        "-6 usec",
        "+7 usecs",
        "-8 microsecond",
        "+9 microseconds",
        "-10 µs",
        "+11 µsec",
        "-12 µsecs",
        "+8 msec -2 µsec",
    ] {
        let mut d = datetime.clone();
        assert!(d.modify(test), "{test}");
        out += &d.format("Y-m-d H:i:s.u");
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2016-10-07 13:25:50.001000
2016-10-07 13:25:49.998000
2016-10-07 13:25:50.003000
2016-10-07 13:25:49.996000
2016-10-07 13:25:50.005000
2016-10-07 13:25:49.999994
2016-10-07 13:25:50.000007
2016-10-07 13:25:49.999992
2016-10-07 13:25:50.000009
2016-10-07 13:25:49.999990
2016-10-07 13:25:50.000011
2016-10-07 13:25:49.999988
2016-10-07 13:25:50.007998"#,
    );
}

/// ext/date/tests/bug65548.phpt ("today"/"tomorrow" use the fixed common::NOW; the
/// comparisons do not depend on it; `===` identity is not ported)
#[test]
fn bug65548() {
    let i_today = DT::new("today", "UTC");
    let i_tomorrow = DT::new("tomorrow", "UTC");
    let m_today = DT::new("today", "UTC");
    let m_tomorrow = DT::new("tomorrow", "UTC");
    let c = |a: &DT, b: &DT| a.t.compare(&b.t);
    assert_eq!(c(&i_today, &i_tomorrow), Ordering::Less);
    assert_eq!(c(&i_today, &m_today), Ordering::Equal);
    assert_eq!(c(&i_today, &m_tomorrow), Ordering::Less);
}

/// ext/date/tests/bug66985.phpt
#[test]
fn bug66985() {
    let mut out = String::new();
    for zone in [
        "CST6CDT",
        "Cuba",
        "Egypt",
        "Eire",
        "EST5EDT",
        "Factory",
        "GB-Eire",
        "GMT0",
        "Greenwich",
        "Hongkong",
        "Iceland",
        "Iran",
        "Israel",
        "Jamaica",
        "Japan",
        "Kwajalein",
        "Libya",
        "MST7MDT",
        "Navajo",
        "NZ-CHAT",
        "Poland",
        "Portugal",
        "PST8PDT",
        "Singapore",
        "Turkey",
        "Universal",
        "W-SU",
        "UTC",
        "GMT",
        "GMT+0100",
        "-0230",
    ] {
        let d = php::timezone_open(zone).unwrap_or_else(|e| panic!("{e}"));
        out += &tz_obj(&d, 1).print_r();
    }
    assert_text(
        &out,
        r#"DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => CST6CDT
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Cuba
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Egypt
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Eire
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => EST5EDT
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Factory
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => GB-Eire
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => GMT0
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Greenwich
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Hongkong
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Iceland
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Iran
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Israel
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Jamaica
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Japan
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Kwajalein
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Libya
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => MST7MDT
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Navajo
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => NZ-CHAT
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Poland
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Portugal
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => PST8PDT
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Singapore
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Turkey
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => Universal
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => W-SU
)
DateTimeZone Object
(
    [timezone_type] => 3
    [timezone] => UTC
)
DateTimeZone Object
(
    [timezone_type] => 2
    [timezone] => GMT
)
DateTimeZone Object
(
    [timezone_type] => 1
    [timezone] => +01:00
)
DateTimeZone Object
(
    [timezone_type] => 1
    [timezone] => -02:30
)"#,
    );
}

/// The timestamps of bug68549-dst-transition-DateTime-setTimezone.phpt.
const BUG68549: &[(&str, &[i64])] = &[
    (
        "America/Los_Angeles",
        &[
            1615687199, 1615687200, 1615687201, 1615690799, 1615690800, 1615690801, 1615708799,
            1615708800, 1615708801, 1615712399, 1615712400, 1615712401, 1615715999, 1615716000,
            1615716001, 1615719599, 1615719600, 1615719601, 1636246799, 1636246800, 1636246801,
            1636250399, 1636250400, 1636250401, 1636268399, 1636268400, 1636268401, 1636271999,
            1636272000, 1636272001, 1636275599, 1636275600, 1636275601, 1636279199, 1636279200,
            1636279201,
        ],
    ),
    (
        "Atlantic/Azores",
        &[
            1616889599, 1616889600, 1616889601, 1616893199, 1616893200, 1616893201, 1616896799,
            1616896800, 1616896801, 1635638399, 1635638400, 1635638401, 1635641999, 1635642000,
            1635642001, 1635645599, 1635645600, 1635645601,
        ],
    ),
    (
        "Europe/London",
        &[
            1616889599, 1616889600, 1616889601, 1616893199, 1616893200, 1616893201, 1616896799,
            1616896800, 1616896801, 1635638399, 1635638400, 1635638401, 1635641999, 1635642000,
            1635642001, 1635645599, 1635645600, 1635645601,
        ],
    ),
    (
        "Europe/Amsterdam",
        &[
            1616885999, 1616886000, 1616886001, 1616889599, 1616889600, 1616889601, 1616893199,
            1616893200, 1616893201, 1616896799, 1616896800, 1616896801, 1616900399, 1616900400,
            1616900401, 1635631199, 1635631200, 1635631201, 1635638399, 1635638400, 1635638401,
            1635641999, 1635642000, 1635642001, 1635645599, 1635645600, 1635645601, 1635649199,
            1635649200, 1635649201,
        ],
    ),
];

/// ext/date/tests/bug68549-dst-transition-DateTime-setTimezone.phpt
#[test]
fn bug68549_dst_transition_datetime_settimezone() {
    let mut out = String::new();
    for (zone, timestamps) in BUG68549 {
        for &timestamp in *timestamps {
            let mut dt = DT::from_format("U", &timestamp.to_string(), None, "UTC").unwrap();
            dt.set_timezone(zone);
            let _ = writeln!(
                out,
                "{zone} {timestamp} -> {} {}",
                dt.timestamp(),
                dt.format(ISO8601)
            );
        }
    }
    assert_text(
        &out,
        r#"America/Los_Angeles 1615687199 -> 1615687199 2021-03-13T17:59:59-0800
America/Los_Angeles 1615687200 -> 1615687200 2021-03-13T18:00:00-0800
America/Los_Angeles 1615687201 -> 1615687201 2021-03-13T18:00:01-0800
America/Los_Angeles 1615690799 -> 1615690799 2021-03-13T18:59:59-0800
America/Los_Angeles 1615690800 -> 1615690800 2021-03-13T19:00:00-0800
America/Los_Angeles 1615690801 -> 1615690801 2021-03-13T19:00:01-0800
America/Los_Angeles 1615708799 -> 1615708799 2021-03-13T23:59:59-0800
America/Los_Angeles 1615708800 -> 1615708800 2021-03-14T00:00:00-0800
America/Los_Angeles 1615708801 -> 1615708801 2021-03-14T00:00:01-0800
America/Los_Angeles 1615712399 -> 1615712399 2021-03-14T00:59:59-0800
America/Los_Angeles 1615712400 -> 1615712400 2021-03-14T01:00:00-0800
America/Los_Angeles 1615712401 -> 1615712401 2021-03-14T01:00:01-0800
America/Los_Angeles 1615715999 -> 1615715999 2021-03-14T01:59:59-0800
America/Los_Angeles 1615716000 -> 1615716000 2021-03-14T03:00:00-0700
America/Los_Angeles 1615716001 -> 1615716001 2021-03-14T03:00:01-0700
America/Los_Angeles 1615719599 -> 1615719599 2021-03-14T03:59:59-0700
America/Los_Angeles 1615719600 -> 1615719600 2021-03-14T04:00:00-0700
America/Los_Angeles 1615719601 -> 1615719601 2021-03-14T04:00:01-0700
America/Los_Angeles 1636246799 -> 1636246799 2021-11-06T17:59:59-0700
America/Los_Angeles 1636246800 -> 1636246800 2021-11-06T18:00:00-0700
America/Los_Angeles 1636246801 -> 1636246801 2021-11-06T18:00:01-0700
America/Los_Angeles 1636250399 -> 1636250399 2021-11-06T18:59:59-0700
America/Los_Angeles 1636250400 -> 1636250400 2021-11-06T19:00:00-0700
America/Los_Angeles 1636250401 -> 1636250401 2021-11-06T19:00:01-0700
America/Los_Angeles 1636268399 -> 1636268399 2021-11-06T23:59:59-0700
America/Los_Angeles 1636268400 -> 1636268400 2021-11-07T00:00:00-0700
America/Los_Angeles 1636268401 -> 1636268401 2021-11-07T00:00:01-0700
America/Los_Angeles 1636271999 -> 1636271999 2021-11-07T00:59:59-0700
America/Los_Angeles 1636272000 -> 1636272000 2021-11-07T01:00:00-0700
America/Los_Angeles 1636272001 -> 1636272001 2021-11-07T01:00:01-0700
America/Los_Angeles 1636275599 -> 1636275599 2021-11-07T01:59:59-0700
America/Los_Angeles 1636275600 -> 1636275600 2021-11-07T01:00:00-0800
America/Los_Angeles 1636275601 -> 1636275601 2021-11-07T01:00:01-0800
America/Los_Angeles 1636279199 -> 1636279199 2021-11-07T01:59:59-0800
America/Los_Angeles 1636279200 -> 1636279200 2021-11-07T02:00:00-0800
America/Los_Angeles 1636279201 -> 1636279201 2021-11-07T02:00:01-0800
Atlantic/Azores 1616889599 -> 1616889599 2021-03-27T22:59:59-0100
Atlantic/Azores 1616889600 -> 1616889600 2021-03-27T23:00:00-0100
Atlantic/Azores 1616889601 -> 1616889601 2021-03-27T23:00:01-0100
Atlantic/Azores 1616893199 -> 1616893199 2021-03-27T23:59:59-0100
Atlantic/Azores 1616893200 -> 1616893200 2021-03-28T01:00:00+0000
Atlantic/Azores 1616893201 -> 1616893201 2021-03-28T01:00:01+0000
Atlantic/Azores 1616896799 -> 1616896799 2021-03-28T01:59:59+0000
Atlantic/Azores 1616896800 -> 1616896800 2021-03-28T02:00:00+0000
Atlantic/Azores 1616896801 -> 1616896801 2021-03-28T02:00:01+0000
Atlantic/Azores 1635638399 -> 1635638399 2021-10-30T23:59:59+0000
Atlantic/Azores 1635638400 -> 1635638400 2021-10-31T00:00:00+0000
Atlantic/Azores 1635638401 -> 1635638401 2021-10-31T00:00:01+0000
Atlantic/Azores 1635641999 -> 1635641999 2021-10-31T00:59:59+0000
Atlantic/Azores 1635642000 -> 1635642000 2021-10-31T00:00:00-0100
Atlantic/Azores 1635642001 -> 1635642001 2021-10-31T00:00:01-0100
Atlantic/Azores 1635645599 -> 1635645599 2021-10-31T00:59:59-0100
Atlantic/Azores 1635645600 -> 1635645600 2021-10-31T01:00:00-0100
Atlantic/Azores 1635645601 -> 1635645601 2021-10-31T01:00:01-0100
Europe/London 1616889599 -> 1616889599 2021-03-27T23:59:59+0000
Europe/London 1616889600 -> 1616889600 2021-03-28T00:00:00+0000
Europe/London 1616889601 -> 1616889601 2021-03-28T00:00:01+0000
Europe/London 1616893199 -> 1616893199 2021-03-28T00:59:59+0000
Europe/London 1616893200 -> 1616893200 2021-03-28T02:00:00+0100
Europe/London 1616893201 -> 1616893201 2021-03-28T02:00:01+0100
Europe/London 1616896799 -> 1616896799 2021-03-28T02:59:59+0100
Europe/London 1616896800 -> 1616896800 2021-03-28T03:00:00+0100
Europe/London 1616896801 -> 1616896801 2021-03-28T03:00:01+0100
Europe/London 1635638399 -> 1635638399 2021-10-31T00:59:59+0100
Europe/London 1635638400 -> 1635638400 2021-10-31T01:00:00+0100
Europe/London 1635638401 -> 1635638401 2021-10-31T01:00:01+0100
Europe/London 1635641999 -> 1635641999 2021-10-31T01:59:59+0100
Europe/London 1635642000 -> 1635642000 2021-10-31T01:00:00+0000
Europe/London 1635642001 -> 1635642001 2021-10-31T01:00:01+0000
Europe/London 1635645599 -> 1635645599 2021-10-31T01:59:59+0000
Europe/London 1635645600 -> 1635645600 2021-10-31T02:00:00+0000
Europe/London 1635645601 -> 1635645601 2021-10-31T02:00:01+0000
Europe/Amsterdam 1616885999 -> 1616885999 2021-03-27T23:59:59+0100
Europe/Amsterdam 1616886000 -> 1616886000 2021-03-28T00:00:00+0100
Europe/Amsterdam 1616886001 -> 1616886001 2021-03-28T00:00:01+0100
Europe/Amsterdam 1616889599 -> 1616889599 2021-03-28T00:59:59+0100
Europe/Amsterdam 1616889600 -> 1616889600 2021-03-28T01:00:00+0100
Europe/Amsterdam 1616889601 -> 1616889601 2021-03-28T01:00:01+0100
Europe/Amsterdam 1616893199 -> 1616893199 2021-03-28T01:59:59+0100
Europe/Amsterdam 1616893200 -> 1616893200 2021-03-28T03:00:00+0200
Europe/Amsterdam 1616893201 -> 1616893201 2021-03-28T03:00:01+0200
Europe/Amsterdam 1616896799 -> 1616896799 2021-03-28T03:59:59+0200
Europe/Amsterdam 1616896800 -> 1616896800 2021-03-28T04:00:00+0200
Europe/Amsterdam 1616896801 -> 1616896801 2021-03-28T04:00:01+0200
Europe/Amsterdam 1616900399 -> 1616900399 2021-03-28T04:59:59+0200
Europe/Amsterdam 1616900400 -> 1616900400 2021-03-28T05:00:00+0200
Europe/Amsterdam 1616900401 -> 1616900401 2021-03-28T05:00:01+0200
Europe/Amsterdam 1635631199 -> 1635631199 2021-10-30T23:59:59+0200
Europe/Amsterdam 1635631200 -> 1635631200 2021-10-31T00:00:00+0200
Europe/Amsterdam 1635631201 -> 1635631201 2021-10-31T00:00:01+0200
Europe/Amsterdam 1635638399 -> 1635638399 2021-10-31T01:59:59+0200
Europe/Amsterdam 1635638400 -> 1635638400 2021-10-31T02:00:00+0200
Europe/Amsterdam 1635638401 -> 1635638401 2021-10-31T02:00:01+0200
Europe/Amsterdam 1635641999 -> 1635641999 2021-10-31T02:59:59+0200
Europe/Amsterdam 1635642000 -> 1635642000 2021-10-31T02:00:00+0100
Europe/Amsterdam 1635642001 -> 1635642001 2021-10-31T02:00:01+0100
Europe/Amsterdam 1635645599 -> 1635645599 2021-10-31T02:59:59+0100
Europe/Amsterdam 1635645600 -> 1635645600 2021-10-31T03:00:00+0100
Europe/Amsterdam 1635645601 -> 1635645601 2021-10-31T03:00:01+0100
Europe/Amsterdam 1635649199 -> 1635649199 2021-10-31T03:59:59+0100
Europe/Amsterdam 1635649200 -> 1635649200 2021-10-31T04:00:00+0100
Europe/Amsterdam 1635649201 -> 1635649201 2021-10-31T04:00:01+0100"#,
    );
}

/// ext/date/tests/bug69587.phpt (partial: `empty()` on the properties)
#[test]
fn bug69587() {
    let interval = DT::new("2009-10-11", "UTC").diff(&DT::new("2009-10-13", "UTC"));
    assert_eq!(interval.diff.m, 0);
    assert_ne!(interval.diff.d, 0);
}

/// ext/date/tests/bug72096.phpt
#[test]
fn bug72096() {
    let mut out = String::new();
    let mut unix = 1461283200;
    while unix <= 1461369600 {
        let _ = writeln!(out, "Time:{}", gmdate("Y-m-d H:i:s = B", unix));
        let _ = writeln!(
            out,
            "Time:{}",
            gmdate("Y-m-d H:i:s = B", unix - 82 * 365 * 24 * 3600)
        );
        unix += 8000;
    }
    assert_text(
        &out,
        r#"Time:2016-04-22 00:00:00 = 041
Time:1934-05-13 00:00:00 = 041
Time:2016-04-22 02:13:20 = 134
Time:1934-05-13 02:13:20 = 134
Time:2016-04-22 04:26:40 = 226
Time:1934-05-13 04:26:40 = 226
Time:2016-04-22 06:40:00 = 319
Time:1934-05-13 06:40:00 = 319
Time:2016-04-22 08:53:20 = 412
Time:1934-05-13 08:53:20 = 412
Time:2016-04-22 11:06:40 = 504
Time:1934-05-13 11:06:40 = 504
Time:2016-04-22 13:20:00 = 597
Time:1934-05-13 13:20:00 = 597
Time:2016-04-22 15:33:20 = 689
Time:1934-05-13 15:33:20 = 689
Time:2016-04-22 17:46:40 = 782
Time:1934-05-13 17:46:40 = 782
Time:2016-04-22 20:00:00 = 875
Time:1934-05-13 20:00:00 = 875
Time:2016-04-22 22:13:20 = 967
Time:1934-05-13 22:13:20 = 967"#,
    );
}

/// ext/date/tests/bug73426.phpt
#[test]
fn bug73426() {
    let mut out = DT::from_format("Y H:i:s z", "2016 12:00:00 15", None, "UTC")
        .expect("createFromFormat")
        .var_dump("DateTime", 1);
    let (format, s) = ("z H:i:s Y", "16 12:00:00 2016");
    assert!(DT::from_format(format, s, None, "UTC").is_none());
    out += &vd_bool(false);
    out += &last_errors(Some(format), s).expect("errors").var_dump();
    assert_text(
        &out,
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2016-01-16 12:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
bool(false)
array(4) {
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [0]=>
    string(57) "A 'day of year' can only come after a year has been found"
  }
}"#,
    );
}

/// ext/date/tests/bug73942.phpt
#[test]
fn bug73942() {
    let mut date = DT::new("2017-01-08", "UTC");
    assert!(date.modify("Friday this week"));
    assert_eq!(date.format("Y-m-d"), "2017-01-06");
}

/// ext/date/tests/bug74652.phpt
#[test]
fn bug74652() {
    let mut out = String::new();
    for format in [
        "2017-03-25 10:52:09",
        "2017-03-25 10:52",
        "2017-03-25 10am",
        "2017-03-25",
        "2017-03",
        "2017.042",
        "2017043",
    ] {
        out += &DT::new(format, "UTC").format("Y-m-d H:i:s");
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2017-03-25 10:52:09
2017-03-25 10:52:00
2017-03-25 10:00:00
2017-03-25 00:00:00
2017-03-01 00:00:00
2017-02-11 00:00:00
2017-02-12 00:00:00"#,
    );
}

/// ext/date/tests/bug75222.phpt
#[test]
fn bug75222() {
    let dt1 = DT::new("2017-01-01T00:00:00.000000Z", "UTC");
    let dt2 = DT::new("2017-01-01T00:00:00.123456Z", "UTC");
    let diff = dt1.diff(&dt2);
    let f = V::Float(diff.diff.us as f64 / 1_000_000.0);
    assert_eq!(f.var_dump(), "float(0.123456)\n");
    assert_eq!(get(&V::Arr(interval_props(&diff)), "f"), Some(&f));
}

/// ext/date/tests/bug76032.phpt
#[test]
fn bug76032() {
    for zone in ["UTC", "Europe/Amsterdam"] {
        let d = DT::new("2008-03-01", zone);
        let a = DT::new("2018-03-01", zone);
        assert_eq!(d.diff(&a).diff.y, 10, "{zone}");
    }
}

/// ext/date/tests/bug77243-002.phpt
#[test]
fn bug77243_002() {
    let starting_points: [i64; 3] = [
        -62167046400 - ((101 * 365) + 25) * 86400,
        -62167046400 - ((100 * 365) + 25) * 86400,
        -62167046400 - ((99 * 365) + 25) * 86400,
    ];
    let mut out = String::new();
    for starting_point in starting_points {
        for i in -7..7 {
            out += &date("Y-m-d o-W-N", starting_point + i * 86400, "UTC");
            out.push('\n');
        }
        out += "\n\n";
    }
    assert_text(
        &out,
        r#"-0102-12-26 -102-52-1
-0102-12-27 -102-52-2
-0102-12-28 -102-52-3
-0102-12-29 -102-52-4
-0102-12-30 -102-52-5
-0102-12-31 -102-52-6
-0101-01-01 -102-52-7
-0101-01-02 -101-01-1
-0101-01-03 -101-01-2
-0101-01-04 -101-01-3
-0101-01-05 -101-01-4
-0101-01-06 -101-01-5
-0101-01-07 -101-01-6
-0101-01-08 -101-01-7


-0101-12-26 -101-52-2
-0101-12-27 -101-52-3
-0101-12-28 -101-52-4
-0101-12-29 -101-52-5
-0101-12-30 -101-52-6
-0101-12-31 -101-52-7
-0100-01-01 -100-01-1
-0100-01-02 -100-01-2
-0100-01-03 -100-01-3
-0100-01-04 -100-01-4
-0100-01-05 -100-01-5
-0100-01-06 -100-01-6
-0100-01-07 -100-01-7
-0100-01-08 -100-02-1


-0100-12-26 -100-52-3
-0100-12-27 -100-52-4
-0100-12-28 -100-52-5
-0100-12-29 -100-52-6
-0100-12-30 -100-52-7
-0100-12-31 -99-01-1
-0099-01-01 -99-01-2
-0099-01-02 -99-01-3
-0099-01-03 -99-01-4
-0099-01-04 -99-01-5
-0099-01-05 -99-01-6
-0099-01-06 -99-01-7
-0099-01-07 -99-02-1
-0099-01-08 -99-02-2"#,
    );
}

/// ext/date/tests/bug80963.phpt
#[test]
fn bug80963() {
    let mut out = String::new();
    let mut london_count = String::new();
    for tzid in ["Europe/London", "America/New_York", "Europe/Berlin"] {
        let t = php::timezone_transitions_get(&tz(tzid), i64::MIN, i64::from(i32::MAX));
        if tzid == "Europe/London" {
            london_count = t.len().to_string();
        }
        out += &V::Int(t.len() as i64).var_dump();
        out += &transition_v(t.last().unwrap()).var_dump();
    }
    // EXPECTF `int(24%d)`.
    assert!(
        london_count.len() >= 3 && london_count.starts_with("24"),
        "{london_count}"
    );
    assert_text(
        &out,
        &r#"int(24%d)
array(5) {
  ["ts"]=>
  int(2140045200)
  ["time"]=>
  string(25) "2037-10-25T01:00:00+00:00"
  ["offset"]=>
  int(0)
  ["isdst"]=>
  bool(false)
  ["abbr"]=>
  string(3) "GMT"
}
int(237)
array(5) {
  ["ts"]=>
  int(2140668000)
  ["time"]=>
  string(25) "2037-11-01T06:00:00+00:00"
  ["offset"]=>
  int(-18000)
  ["isdst"]=>
  bool(false)
  ["abbr"]=>
  string(3) "EST"
}
int(144)
array(5) {
  ["ts"]=>
  int(2140045200)
  ["time"]=>
  string(25) "2037-10-25T01:00:00+00:00"
  ["offset"]=>
  int(3600)
  ["isdst"]=>
  bool(false)
  ["abbr"]=>
  string(3) "CET"
}"#
        .replace("int(24%d)", &format!("int({london_count})")),
    );
}

/// ext/date/tests/bug81273.phpt
#[test]
fn bug81273() {
    let time = "2000-01-01 00:00:00.000000";
    let auz = DT::new_in(time, "Australia/Sydney", "UTC");
    let us = DT::new_in(time, "America/Los_Angeles", "UTC");
    assert_eq!(auz.diff(&us).diff.h, 19);
}

/// ext/date/tests/checkdate_basic1.phpt
#[test]
fn checkdate_basic1() {
    let valid = [
        (1, 1, 2009),
        (12, 31, 2009),
        (7, 2, 1963),
        (5, 31, 2009),
        (2, 28, 2009),
        (2, 29, 2008),
        (7, 2, 1),
        (7, 2, 32767),
    ];
    let invalid = [
        (13, 1, 2009),
        (2, 31, 2009),
        (1, 32, 2009),
        (2, 29, 2009),
        (7, 2, 32768),
        (7, 2, 0),
    ];
    for (m, d, y) in valid {
        assert!(checkdate(m, d, y), "{m} {d} {y}");
    }
    for (m, d, y) in invalid {
        assert!(!checkdate(m, d, y), "{m} {d} {y}");
    }
}

/// ext/date/tests/date-set-timestamp.phpt (`new DateTime()` uses the fixed
/// common::NOW; the result does not depend on it)
#[test]
fn date_set_timestamp() {
    let zone = "Europe/Oslo";
    let d = DT::new("@1217184864", zone);
    let mut out = d.format("Y-m-d H:i e\n");
    let mut d = DT::new("now", zone);
    d.set_timestamp(1217184864);
    out += &d.format("Y-m-d H:i e\n");
    assert_text(
        &out,
        r#"2008-07-27 18:54 +00:00
2008-07-27 20:54 Europe/Oslo"#,
    );
}

/// ext/date/tests/date_create-1.phpt
#[test]
fn date_create_1() {
    let zone = "Europe/Oslo";
    let s = "2005-07-14 22:30:41";
    let mut d = Vec::new();
    for suffix in [
        "",
        " GMT",
        " CET",
        " CEST",
        " Europe/Oslo",
        " America/Los_Angeles",
    ] {
        d.push(DT::new(&format!("{s}{suffix}"), zone));
    }
    for (suffix, tz) in [
        ("", "GMT"),
        ("", "Europe/London"),
        ("", "America/Los_Angeles"),
        (" GMT", "GMT"),
        (" GMT", "Europe/London"),
        (" GMT", "America/Los_Angeles"),
        (" Europe/Oslo", "GMT"),
        (" America/Los_Angeles", "Europe/London"),
    ] {
        d.push(DT::new_in(&format!("{s}{suffix}"), tz, zone));
    }
    let mut out = String::new();
    for date in &d {
        out += &date.format(ISO8601);
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2005-07-14T22:30:41+0200
2005-07-14T22:30:41+0000
2005-07-14T22:30:41+0100
2005-07-14T22:30:41+0200
2005-07-14T22:30:41+0200
2005-07-14T22:30:41-0700
2005-07-14T22:30:41+0000
2005-07-14T22:30:41+0100
2005-07-14T22:30:41-0700
2005-07-14T22:30:41+0000
2005-07-14T22:30:41+0000
2005-07-14T22:30:41+0000
2005-07-14T22:30:41+0200
2005-07-14T22:30:41-0700"#,
    );
}

/// ext/date/tests/date_diff.phpt
#[test]
fn date_diff() {
    const COUNT: usize = 120;
    let d0 = DT::new("2009-11-20", "UTC");
    let dates: Vec<DT> = (0..COUNT * 12)
        .map(|i| {
            let mut d = d0.clone();
            d.add(&format!("P{i}D"));
            d
        })
        .collect();
    let mut ok = 0;
    let mut fails = Vec::new();
    for i in 0..COUNT {
        for j in 0..COUNT * 12 {
            let diff = dates[i].diff(&dates[j]);
            let mut current = dates[i].clone();
            let int = php::date_interval_create(&interval_format(&diff, "P%yY%mM%dD")).unwrap();
            if current.t.compare(&dates[j].t) == Ordering::Greater {
                current.sub_interval(&int);
            } else {
                current.add_interval(&int);
            }
            if current.t.compare(&dates[j].t) != Ordering::Equal {
                if fails.len() < 10 {
                    fails.push(format!(
                        "FAIL: {} + {} = {} ({})",
                        dates[i].format("Y-m-d"),
                        interval_format(&int, "%y-%m-%d"),
                        current.format("Y-m-d"),
                        dates[j].format("Y-m-d")
                    ));
                }
            } else {
                ok += 1;
            }
        }
    }
    assert_eq!(ok, 172800, "{fails:#?}");
}

/// ext/date/tests/date_interval_create_from_date_string.phpt
#[test]
fn date_interval_create_from_date_string() {
    let i = |s: &str| php::date_interval_create_from_date_string(s).unwrap().diff;
    assert_eq!(i("1 day").d, 1);
    assert_eq!(i("2 weeks").d, 14);
    assert_eq!(i("3 months").m, 3);
    assert_eq!(i("4 years").y, 4);
    let r = i("1 year + 1 day");
    assert_eq!((r.y, r.d), (1, 1));
}

/// ext/date/tests/date_interval_set_state_error1.phpt (partial: the interval and
/// the strtotime() error that `__unserialize()` reports)
#[test]
fn date_interval_set_state_error1() {
    let interval = php::date_interval_create("P1Y1DT1H1M").unwrap();
    let dump = interval_var_dump(&interval, 1);
    let r = php_timelib::strtotime("wrong", &php_timelib::builtin_tzinfo);
    let msg = parse_error(
        "Unknown or bad format",
        "wrong",
        " while unserializing",
        &r.errors,
    );
    let out = format!("{dump}Error: {msg}\n{dump}");
    assert_text(
        &out,
        &r#"object(DateInterval)#%d (%d) {
  ["y"]=>
  int(1)
  ["m"]=>
  int(0)
  ["d"]=>
  int(1)
  ["h"]=>
  int(1)
  ["i"]=>
  int(1)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  bool(false)
  ["from_string"]=>
  bool(false)
}
Error: Unknown or bad format (wrong) at position 0 (w) while unserializing: The timezone could not be found in the database
object(DateInterval)#%d (%d) {
  ["y"]=>
  int(1)
  ["m"]=>
  int(0)
  ["d"]=>
  int(1)
  ["h"]=>
  int(1)
  ["i"]=>
  int(1)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  bool(false)
  ["from_string"]=>
  bool(false)
}"#.replace("#%d (%d)", "#1 (10)"),
    );
}

/// ext/date/tests/date_parse_001.phpt
#[test]
fn date_parse_001() {
    let mut out = String::new();
    for s in [
        "2006-12-12 10:00:00.5",
        "2006-12-12",
        "2006-12--12",
        "2006-02-30",
        "2006-03-04",
        "2006-03",
        "03-03",
        "0-0",
        "",
    ] {
        out += &date_parse(s).var_dump();
    }
    out += "Done\n";
    assert_text(
        &out,
        r#"array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(12)
  ["day"]=>
  int(12)
  ["hour"]=>
  int(10)
  ["minute"]=>
  int(0)
  ["second"]=>
  int(0)
  ["fraction"]=>
  float(0.5)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(12)
  ["day"]=>
  int(12)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(15) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(12)
  ["day"]=>
  int(1)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [7]=>
    string(20) "Unexpected character"
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(1)
  ["zone"]=>
  int(-43200)
  ["is_dst"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(2)
  ["day"]=>
  int(30)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [11]=>
    string(27) "The parsed date was invalid"
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(3)
  ["day"]=>
  int(4)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  int(2006)
  ["month"]=>
  int(3)
  ["day"]=>
  int(1)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
array(15) {
  ["year"]=>
  bool(false)
  ["month"]=>
  bool(false)
  ["day"]=>
  bool(false)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(2)
  ["errors"]=>
  array(2) {
    [0]=>
    string(20) "Unexpected character"
    [1]=>
    string(20) "Unexpected character"
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(1)
  ["zone"]=>
  int(-10800)
  ["is_dst"]=>
  bool(false)
}
array(15) {
  ["year"]=>
  bool(false)
  ["month"]=>
  bool(false)
  ["day"]=>
  bool(false)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [0]=>
    string(20) "Unexpected character"
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(1)
  ["zone"]=>
  int(0)
  ["is_dst"]=>
  bool(false)
}
array(12) {
  ["year"]=>
  bool(false)
  ["month"]=>
  bool(false)
  ["day"]=>
  bool(false)
  ["hour"]=>
  bool(false)
  ["minute"]=>
  bool(false)
  ["second"]=>
  bool(false)
  ["fraction"]=>
  bool(false)
  ["warning_count"]=>
  int(0)
  ["warnings"]=>
  array(0) {
  }
  ["error_count"]=>
  int(1)
  ["errors"]=>
  array(1) {
    [0]=>
    string(12) "Empty string"
  }
  ["is_localtime"]=>
  bool(false)
}
Done"#,
    );
}

/// ext/date/tests/date_period_exclude_start_and_include_end.phpt (DatePeriod
/// iteration emulated with `date_period_advance()` steps)
#[test]
fn date_period_exclude_start_and_include_end() {
    let start = DT::new("2010-06-07", "UTC");
    let end = DT::new("2010-06-10", "UTC");
    let interval = php::date_interval_create("P1D").unwrap();
    let mut current = start.clone();
    // EXCLUDE_START_DATE
    period_advance(&mut current.t, &interval);
    let mut out = String::new();
    // INCLUDE_END_DATE
    while current.t.compare(&end.t) != Ordering::Greater {
        out += &current.format("Y-m-d");
        out.push('\n');
        period_advance(&mut current.t, &interval);
    }
    assert_text(
        &out,
        r#"2010-06-08
2010-06-09
2010-06-10"#,
    );
}

/// ext/date/tests/date_sun_info_001.phpt
#[test]
fn date_sun_info_001() {
    let si = sun_info(st("2006-12-12", "UTC"), 31.7667, 35.2333, &tz("UTC"));
    let v = |s: SunValue| match s {
        SunValue::Time(t) => V::Int(t),
        SunValue::Always(b) => V::Bool(b),
    };
    let arr = V::Arr(vec![
        (sk("sunrise"), v(si.sunrise)),
        (sk("sunset"), v(si.sunset)),
        (sk("transit"), V::Int(si.transit)),
        (sk("civil_twilight_begin"), v(si.civil_twilight_begin)),
        (sk("civil_twilight_end"), v(si.civil_twilight_end)),
        (sk("nautical_twilight_begin"), v(si.nautical_twilight_begin)),
        (sk("nautical_twilight_end"), v(si.nautical_twilight_end)),
        (
            sk("astronomical_twilight_begin"),
            v(si.astronomical_twilight_begin),
        ),
        (
            sk("astronomical_twilight_end"),
            v(si.astronomical_twilight_end),
        ),
    ]);
    let out = format!("{}Done\n", arr.var_dump());
    assert_text(
        &out,
        r#"array(9) {
  ["sunrise"]=>
  int(1165897761)
  ["sunset"]=>
  int(1165934160)
  ["transit"]=>
  int(1165915961)
  ["civil_twilight_begin"]=>
  int(1165896156)
  ["civil_twilight_end"]=>
  int(1165935765)
  ["nautical_twilight_begin"]=>
  int(1165894334)
  ["nautical_twilight_end"]=>
  int(1165937588)
  ["astronomical_twilight_begin"]=>
  int(1165892551)
  ["astronomical_twilight_end"]=>
  int(1165939371)
}
Done"#,
    );
}

/// ext/date/tests/date_sunrise_variation8.phpt
#[test]
fn date_sunrise_variation8() {
    let zone = "Africa/Casablanca";
    let time_date = [
        ("12 Aug 2008", mktime(8, 8, 8, 8, 12, 2008, zone)),
        ("13 Aug 2008", mktime(8, 8, 8, 8, 13, 2008, zone)),
        ("12 Nov 2008", mktime(8, 8, 8, 11, 12, 2008, zone)),
        ("13 Nov 2008", mktime(8, 8, 8, 11, 13, 2008, zone)),
    ];
    let mut out = String::from("*** Testing date_sunrise() : usage variation ***\n");
    for (d, time) in time_date {
        let _ = writeln!(out, "\n--{d}--");
        for lat in [90.0, -90.0] {
            for format in [SunFormat::String, SunFormat::Double, SunFormat::Timestamp] {
                // date.sunrise_zenith defaults to 90.833333.
                let r = sunrise_sunset(time, format, lat, 0.0, 90.833333, None, &tz(zone), false);
                out += &match r {
                    None => vd_bool(false),
                    Some(r) => format!("{r:?}\n"),
                };
            }
        }
    }
    assert_text(
        &out,
        r#"*** Testing date_sunrise() : usage variation ***

--12 Aug 2008--
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)

--13 Aug 2008--
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)

--12 Nov 2008--
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)

--13 Nov 2008--
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)
bool(false)"#,
    );
}

/// ext/date/tests/date_time_immutable-inherited.phpt
#[test]
fn date_time_immutable_inherited() {
    let current = "2012-12-27 16:24:08";
    let v = DT::new(current, "Europe/London");
    let mut out = format!("\ngetTimezone():\n{}", vd_str(&v.timezone_name()));
    let v = DT::new(current, "Europe/London");
    let _ = write!(
        out,
        "\ngetTimestamp():\n{}",
        V::Int(v.timestamp()).var_dump()
    );
    assert_text(
        out.trim_start(),
        r#"getTimezone():
string(13) "Europe/London"

getTimestamp():
int(1356625448)"#,
    );
}

/// ext/date/tests/date_timestamp_set_nullparam2.phpt (partial: `null` is taken as
/// 0; the deprecation is not ported; `date_create()` uses the fixed common::NOW)
#[test]
fn date_timestamp_set_nullparam2() {
    let mut dtms021 = DT::new("now", "UTC");
    dtms021.set_timestamp(0);
    assert_text(
        &dtms021.var_dump("DateTime", 1),
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#,
    );
}

/// ext/date/tests/forward-transition-construction.phpt
#[test]
fn forward_transition_construction() {
    let zone = "America/New_York";
    let mut out = String::new();
    for s in [
        "2010-03-14 01:30:00",
        "2010-03-14 02:00:00",
        "2010-03-14 02:30:00",
        "2010-03-14 03:00:00",
        "2010-03-14 03:30:00",
    ] {
        out += &DT::new(s, zone).format("Y-m-d H:i:s T/e - U");
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2010-03-14 01:30:00 EST/America/New_York - 1268548200
2010-03-14 03:00:00 EDT/America/New_York - 1268550000
2010-03-14 03:30:00 EDT/America/New_York - 1268551800
2010-03-14 03:00:00 EDT/America/New_York - 1268550000
2010-03-14 03:30:00 EDT/America/New_York - 1268551800"#,
    );
}

/// ext/date/tests/getdate_variation5.phpt
#[test]
fn getdate_variation5() {
    let mut out = String::from("*** Testing getdate() : usage variation ***\n");
    for timezone in [
        "Pacific/Samoa",
        "US/Alaska",
        "Africa/Casablanca",
        "Europe/Moscow",
        "Asia/Hong_Kong",
        "Australia/Brisbane",
        "Pacific/Wallis",
    ] {
        let _ = writeln!(out, "\n--{timezone}--");
        out += &getdate(0, timezone).var_dump();
    }
    assert_text(
        &out,
        r#"*** Testing getdate() : usage variation ***

--Pacific/Samoa--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(13)
  ["mday"]=>
  int(31)
  ["wday"]=>
  int(3)
  ["mon"]=>
  int(12)
  ["year"]=>
  int(1969)
  ["yday"]=>
  int(364)
  ["weekday"]=>
  string(9) "Wednesday"
  ["month"]=>
  string(8) "December"
  [0]=>
  int(0)
}

--US/Alaska--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(14)
  ["mday"]=>
  int(31)
  ["wday"]=>
  int(3)
  ["mon"]=>
  int(12)
  ["year"]=>
  int(1969)
  ["yday"]=>
  int(364)
  ["weekday"]=>
  string(9) "Wednesday"
  ["month"]=>
  string(8) "December"
  [0]=>
  int(0)
}

--Africa/Casablanca--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(0)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(4)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1970)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(8) "Thursday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(0)
}

--Europe/Moscow--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(3)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(4)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1970)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(8) "Thursday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(0)
}

--Asia/Hong_Kong--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(8)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(4)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1970)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(8) "Thursday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(0)
}

--Australia/Brisbane--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(10)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(4)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1970)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(8) "Thursday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(0)
}

--Pacific/Wallis--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(0)
  ["hours"]=>
  int(12)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(4)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1970)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(8) "Thursday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(0)
}"#,
    );
}

/// ext/date/tests/gh10583.phpt
#[test]
fn gh10583() {
    let mut out = String::new();
    for _ in 0..2 {
        let mut dt = DT::new("2015-01-01 00:00:00+00:00", "UTC");
        out += &vd_str(&dt.format("c"));
        assert!(dt.modify("+1 s"));
        out += &vd_str(&dt.format("c"));
    }
    assert_text(
        &out,
        r#"string(25) "2015-01-01T00:00:00+00:00"
string(25) "2015-01-01T00:00:00+00:00"
string(25) "2015-01-01T00:00:00+00:00"
string(25) "2015-01-01T00:00:00+00:00""#,
    );
}

/// ext/date/tests/gh11281.phpt
#[test]
fn gh11281() {
    let mut out = String::new();
    for z in [
        "+03:00",
        "+03:00:00",
        "-03:00:00",
        "+03:00:01",
        "-03:00:01",
        "+03:00:58",
        "-03:00:58",
        "+03:00:59",
        "-03:00:59",
    ] {
        out += &php::timezone_open(z).unwrap().name();
        out.push('\n');
    }
    assert_text(
        &out,
        r#"+03:00
+03:00
-03:00
+03:00:01
-03:00:01
+03:00:58
-03:00:58
+03:00:59
-03:00:59"#,
    );
}

/// ext/date/tests/gh16454.phpt (the deprecation notices are not ported)
#[test]
fn gh16454() {
    let utc = tz("UTC");
    for calc_sunset in [false, true] {
        for gmt_offset in [f64::MAX, -f64::MAX] {
            let r = sunrise_sunset(
                0,
                SunFormat::String,
                61.0,
                -150.0,
                90.0,
                Some(gmt_offset),
                &utc,
                calc_sunset,
            );
            assert_eq!(r, None, "sunset={calc_sunset} offset={gmt_offset}");
        }
    }
}

/// ext/date/tests/gh7758.phpt
#[test]
fn gh7758() {
    let mut out = String::new();
    for ts in ["0.4", "0", "-0.4", "-1", "-1.4"] {
        let date = DT::new(&format!("@{ts}"), "UTC");
        out += &V::Obj("DateTime".into(), 1, date.props()).print_r();
    }
    assert_text(
        &out,
        r#"DateTime Object
(
    [date] => 1970-01-01 00:00:00.400000
    [timezone_type] => 1
    [timezone] => +00:00
)
DateTime Object
(
    [date] => 1970-01-01 00:00:00.000000
    [timezone_type] => 1
    [timezone] => +00:00
)
DateTime Object
(
    [date] => 1969-12-31 23:59:59.600000
    [timezone_type] => 1
    [timezone] => +00:00
)
DateTime Object
(
    [date] => 1969-12-31 23:59:59.000000
    [timezone_type] => 1
    [timezone] => +00:00
)
DateTime Object
(
    [date] => 1969-12-31 23:59:58.600000
    [timezone_type] => 1
    [timezone] => +00:00
)"#,
    );
}

/// ext/date/tests/gh9891.phpt
#[test]
fn gh9891() {
    let mut m = DT::new_in("2022-12-20 14:30:25", "Europe/Paris", "UTC");
    assert!(m.modify("@1234567890"));
    let mut out = V::Int(m.timestamp()).var_dump();
    out += "=======\n";
    let mut a = DT::new_in("2022-11-01 13:30:00", "America/Lima", "UTC");
    let mut b = a.clone();
    let _ = writeln!(out, "$a: {}", a.format(ATOM));
    let _ = writeln!(out, "$b: {}", b.format(ATOM));
    let _ = writeln!(out, "$a: @{}", a.timestamp());
    let _ = writeln!(out, "$b: setTimestamp({})", b.timestamp());
    assert!(a.modify(&format!("@{}", a.timestamp())));
    b.set_timestamp(b.timestamp());
    let _ = writeln!(out, "$a: {}", a.format(ATOM));
    let _ = writeln!(out, "$b: {}", b.format(ATOM));
    assert_text(
        &out,
        r#"int(1234567890)
=======
$a: 2022-11-01T13:30:00-05:00
$b: 2022-11-01T13:30:00-05:00
$a: @1667327400
$b: setTimestamp(1667327400)
$a: 2022-11-01T18:30:00+00:00
$b: 2022-11-01T13:30:00-05:00"#,
    );
}

/// ext/date/tests/gmdate_variation13.phpt (partial: `gmdate()` without a timestamp
/// is not checked; its EXPECTF placeholder line is echoed literally)
#[test]
fn gmdate_variation13() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    for (key, value) in [
        ("DATE_ATOM", ATOM),
        ("DATE_COOKIE", "l, d-M-Y H:i:s T"),
        ("DATE_RFC822", "D, d M y H:i:s O"),
        ("DATE_RFC850", "l, d-M-y H:i:s T"),
        ("DATE_RFC1036", "D, d M y H:i:s O"),
        ("DATE_RFC1123", "D, d M Y H:i:s O"),
        ("DATE_RFC2822", "D, d M Y H:i:s O"),
        ("DATE_RFC3339", "Y-m-d\\TH:i:sP"),
        ("DATE_RSS", "D, d M Y H:i:s O"),
        ("DATE_W3C", "Y-m-d\\TH:i:sP"),
    ] {
        let _ = writeln!(out, "\n--{key} Constant--");
        out += &vd_str(&gmdate(value, timestamp));
        out += "string(%d) \"%s\"\n";
    }
    assert_text(
        &out,
        r#"*** Testing gmdate() : usage variation ***

--DATE_ATOM Constant--
string(25) "2008-08-08T08:08:08+00:00"
string(%d) "%s"

--DATE_COOKIE Constant--
string(32) "Friday, 08-Aug-2008 08:08:08 GMT"
string(%d) "%s"

--DATE_RFC822 Constant--
string(29) "Fri, 08 Aug 08 08:08:08 +0000"
string(%d) "%s"

--DATE_RFC850 Constant--
string(30) "Friday, 08-Aug-08 08:08:08 GMT"
string(%d) "%s"

--DATE_RFC1036 Constant--
string(29) "Fri, 08 Aug 08 08:08:08 +0000"
string(%d) "%s"

--DATE_RFC1123 Constant--
string(31) "Fri, 08 Aug 2008 08:08:08 +0000"
string(%d) "%s"

--DATE_RFC2822 Constant--
string(31) "Fri, 08 Aug 2008 08:08:08 +0000"
string(%d) "%s"

--DATE_RFC3339 Constant--
string(25) "2008-08-08T08:08:08+00:00"
string(%d) "%s"

--DATE_RSS Constant--
string(31) "Fri, 08 Aug 2008 08:08:08 +0000"
string(%d) "%s"

--DATE_W3C Constant--
string(25) "2008-08-08T08:08:08+00:00"
string(%d) "%s""#,
    );
}

/// ext/date/tests/gmdate_variation9.phpt (partial: `gmdate()` without a timestamp
/// is not checked)
#[test]
fn gmdate_variation9() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let cases = [
        ("a", "string(2) \"am\""),
        ("a", "string(2) \"am\""),
        ("B", "string(3) \"380\""),
        ("g", "string(1) \"8\""),
        ("G", "string(1) \"8\""),
        ("h", "string(2) \"08\""),
        ("H", "string(2) \"08\""),
        ("i", "string(2) \"08\""),
        ("s", "string(2) \"08\""),
        ("u", "string(6) \"000000\""),
    ];
    for (format, expected) in cases {
        assert_eq!(vd_str(&gmdate(format, timestamp)).trim_end(), expected);
    }
}

/// ext/date/tests/idate_variation4.phpt (`idate()` without a timestamp: the fixed
/// common::NOW is used, only the `int(%d)` shape is checked)
#[test]
fn idate_variation4() {
    for c in ['d', 'L', 'm', 't', 'w', 'W', 'y', 'Y', 'z'] {
        assert!(idate(c, NOW, "Asia/Calcutta").is_some(), "{c}");
    }
}

/// ext/date/tests/localtime_basic.phpt (partial: `localtime()` without a timestamp
/// is not checked)
#[test]
fn localtime_basic() {
    let mut out = localtime(10, "UTC", true).var_dump();
    out += &localtime(10, "UTC", false).var_dump();
    assert_text(
        &out,
        r#"array(9) {
  ["tm_sec"]=>
  int(10)
  ["tm_min"]=>
  int(0)
  ["tm_hour"]=>
  int(0)
  ["tm_mday"]=>
  int(1)
  ["tm_mon"]=>
  int(0)
  ["tm_year"]=>
  int(70)
  ["tm_wday"]=>
  int(4)
  ["tm_yday"]=>
  int(0)
  ["tm_isdst"]=>
  int(0)
}
array(9) {
  [0]=>
  int(10)
  [1]=>
  int(0)
  [2]=>
  int(0)
  [3]=>
  int(1)
  [4]=>
  int(0)
  [5]=>
  int(70)
  [6]=>
  int(4)
  [7]=>
  int(0)
  [8]=>
  int(0)
}"#,
    );
}

/// ext/date/tests/mktime-3.phpt (a 32-bit test: only the lines that are not "out
/// of range" there are checked, as they are the same on 64-bit)
#[test]
fn mktime_3() {
    let mut zone = "";
    let mut checked = 0;
    for line in r#"America/Toronto
Y:    0 - January 2000-01-01T01:01:01-0500
Y:   69 - out of range
Y:   70 - January 1970-01-01T01:01:01-0500
Y:   71 - January 1971-01-01T01:01:01-0500
Y:   99 - January 1999-01-01T01:01:01-0500
Y:  100 - January 2000-01-01T01:01:01-0500
Y:  105 - out of range
Y: 1900 - out of range
Y: 1901 - out of range
Y: 1902 - January 1902-01-01T01:01:01-0500
Y: 1999 - January 1999-01-01T01:01:01-0500
Y: 2000 - January 2000-01-01T01:01:01-0500
Y: 2001 - January 2001-01-01T01:01:01-0500

Europe/Oslo
Y:    0 - January 2000-01-01T01:01:01+0100
Y:   69 - out of range
Y:   70 - January 1970-01-01T01:01:01+0100
Y:   71 - January 1971-01-01T01:01:01+0100
Y:   99 - January 1999-01-01T01:01:01+0100
Y:  100 - January 2000-01-01T01:01:01+0100
Y:  105 - out of range
Y: 1900 - out of range
Y: 1901 - out of range
Y: 1902 - January 1902-01-01T01:01:01+0100
Y: 1999 - January 1999-01-01T01:01:01+0100
Y: 2000 - January 2000-01-01T01:01:01+0100
Y: 2001 - January 2001-01-01T01:01:01+0100"#
        .lines()
    {
        if let Some(rest) = line.strip_prefix("Y: ") {
            if line.ends_with("out of range") {
                continue;
            }
            let year: i64 = rest[..4].trim().parse().unwrap();
            let ret = mktime(1, 1, 1, 1, 1, year, zone);
            let got = format!("Y: {year:4} - {}", date(&format!("F {ISO8601}"), ret, zone));
            assert_eq!(got, line);
            checked += 1;
        } else if !line.is_empty() {
            zone = line;
        }
    }
    assert_eq!(checked, 18);
}

/// ext/date/tests/oom_date_create.phpt
#[test]
fn oom_date_create() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    assert!(DT::try_new(&s, None, "UTC").is_err());
}

/// ext/date/tests/oom_dateinterval_createfromdatestring.phpt
#[test]
fn oom_dateinterval_createfromdatestring() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    assert!(php::date_interval_create_from_date_string(&s).is_none());
    let r = php_timelib::strtotime(&s, &php_timelib::builtin_tzinfo);
    let e = &r.errors.errors[0];
    assert_eq!(
        (e.position, e.character, e.message),
        (0, 1, "Unexpected character")
    );
}

/// ext/date/tests/oom_strtotime.phpt
#[test]
fn oom_strtotime() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    assert_eq!(strtotime_abs(&s, "UTC"), None);
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type3-bs.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type3_bs() {
    let zone = "America/New_York";
    let date_format = "Y-m-d H:i:s T e";
    let mut out = String::new();
    for (label, s, set_tz, interval_spec) in [
        ("bs1", "2010-11-07 01:00:00 EST", true, "PT1S"),
        ("bs2", "2010-11-07 04:30:00", false, "P1D"),
        ("bs3", "2010-11-07 03:30:00", false, "PT24H"),
        ("bs4", "2010-11-07 02:30:00", false, "PT23H"),
        ("bs5", "2010-11-07 01:30:00 EST", true, "PT22H"),
        ("bs6", "2010-11-07 01:30:00 EDT", true, "PT21H"),
        ("bs7", "2010-11-07 01:30:00", false, "P1D"),
        ("bs8", "2010-11-07 01:30:00 EST", true, "P1DT1H"),
        ("bs9", "2010-11-07 03:30:00", false, "P1D"),
        ("bs10", "2010-11-07 02:30:00", false, "P1D"),
    ] {
        let mut end = DT::new(s, zone);
        if set_tz {
            end.set_timezone(zone);
        }
        let before = end.format(date_format);
        end.sub(interval_spec);
        let _ = writeln!(
            out,
            "{label} {before} - {interval_spec} = {}",
            end.format(date_format)
        );
    }
    assert_text(
        &out,
        r#"bs1 2010-11-07 01:00:00 EST America/New_York - PT1S = 2010-11-07 01:59:59 EDT America/New_York
bs2 2010-11-07 04:30:00 EST America/New_York - P1D = 2010-11-06 04:30:00 EDT America/New_York
bs3 2010-11-07 03:30:00 EST America/New_York - PT24H = 2010-11-06 04:30:00 EDT America/New_York
bs4 2010-11-07 02:30:00 EST America/New_York - PT23H = 2010-11-06 04:30:00 EDT America/New_York
bs5 2010-11-07 01:30:00 EST America/New_York - PT22H = 2010-11-06 04:30:00 EDT America/New_York
bs6 2010-11-07 01:30:00 EDT America/New_York - PT21H = 2010-11-06 04:30:00 EDT America/New_York
bs7 2010-11-07 01:30:00 EDT America/New_York - P1D = 2010-11-06 01:30:00 EDT America/New_York
bs8 2010-11-07 01:30:00 EST America/New_York - P1DT1H = 2010-11-06 00:30:00 EDT America/New_York
bs9 2010-11-07 03:30:00 EST America/New_York - P1D = 2010-11-06 03:30:00 EDT America/New_York
bs10 2010-11-07 02:30:00 EST America/New_York - P1D = 2010-11-06 02:30:00 EDT America/New_York"#,
    );
}

/// ext/date/tests/strtotime-mysql.phpt (a 32-bit test: the overflow case is not
/// checked)
#[test]
fn strtotime_mysql() {
    let mut out = String::new();
    for d in ["19970523091528", "20001231185859"] {
        out += &vd_str(&date("r", st(d, "UTC"), "UTC"));
    }
    assert_text(
        &out,
        r#"string(31) "Fri, 23 May 1997 09:15:28 +0000"
string(31) "Sun, 31 Dec 2000 18:58:59 +0000""#,
    );
}

/// ext/date/tests/strtotime_basic.phpt
#[test]
fn strtotime_basic() {
    let mut out = String::new();
    for s in [
        "1 Monday December 2008",
        "2 Monday December 2008",
        "3 Monday December 2008",
        "first Monday December 2008",
        "second Monday December 2008",
        "third Monday December 2008",
    ] {
        out += &vd_str(&date("Y-m-d", st(s, "UTC"), "UTC"));
    }
    assert_text(
        &out,
        r#"string(10) "2008-12-01"
string(10) "2008-12-08"
string(10) "2008-12-15"
string(10) "2008-12-08"
string(10) "2008-12-15"
string(10) "2008-12-22""#,
    );
}

/// ext/date/tests/timezone_location_get.phpt
#[test]
fn timezone_location_get() {
    let location = &tz("Europe/Oslo").location;
    assert_eq!(location.country_code, "NO");
    // EXPECTF `float(59.9166%d)`.
    let lat = V::Float(location.latitude).var_dump();
    assert!(lat.starts_with("float(59.9166"), "{lat}");
    assert_eq!(V::Float(location.longitude).var_dump(), "float(10.75)\n");
    assert_eq!(location.comments, "");
}

/// ext/date/tests/timezone_transitions_get_basic1.phpt
#[test]
fn timezone_transitions_get_basic1() {
    let tran = php::timezone_transitions_get(&tz("Europe/London"), -306972000, -37241999);
    assert_eq!(tran.len(), 18);
    assert_text(
        &transition_v(&tran[6]).var_dump(),
        r#"array(5) {
  ["ts"]=>
  int(-213228000)
  ["time"]=>
  string(25) "1963-03-31T02:00:00+00:00"
  ["offset"]=>
  int(3600)
  ["isdst"]=>
  bool(true)
  ["abbr"]=>
  string(3) "BST"
}"#,
    );
}
