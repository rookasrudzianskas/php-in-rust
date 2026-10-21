//! Tests ported from `ext/date/tests/*.phpt` (group 5).
//!
//! Expected outputs are copied verbatim from the phpt files (see the `EXP_*`
//! constants at the end of the file). For `--EXPECTF--` sections only the
//! deterministic parts are asserted (via [`assert_textf`]).
mod common;
use common::*;

use php_timelib::php::{self, PhpInterval, PhpTimeZone};
use php_timelib::{
    IsoInterval, SunFormat, SunResult, Time, TzLookupEntry, ZoneType, builtin_tzinfo, day_of_week,
    day_of_year, strtointerval, sunrise_sunset, timezone_abbreviations_list, timezone_id_is_valid,
};

// ---------------------------------------------------------------------------------
// Helpers

/// `var_dump($string)`.
fn vstr(s: &str) -> String {
    V::Str(s.to_string()).var_dump()
}

/// A string array key.
fn k(s: &str) -> K {
    K::S(s.to_string())
}

/// Matches one line against a run-tests.php `EXPECTF` pattern (`%s`, `%S`, `%d`,
/// `%i`, `%f`, `%c`, `%%`).
fn line_matches(a: &[u8], p: &[u8]) -> bool {
    if p.is_empty() {
        return a.is_empty();
    }
    if p[0] == b'%' && p.len() >= 2 {
        let rest = &p[2..];
        let is_float = |s: &[u8]| {
            let st = std::str::from_utf8(s).unwrap_or("");
            let t = st.strip_prefix(['+', '-']).unwrap_or(st);
            let t = t.split(['e', 'E']).collect::<Vec<_>>();
            if t.len() > 2 {
                return false;
            }
            if t.len() == 2 {
                let e = t[1].strip_prefix(['+', '-']).unwrap_or(t[1]);
                if e.is_empty() || !e.bytes().all(|c| c.is_ascii_digit()) {
                    return false;
                }
            }
            let m = t[0].strip_prefix('.').unwrap_or(t[0]);
            let mut parts = m.splitn(2, '.');
            let ip = parts.next().unwrap_or("");
            let fp = parts.next().unwrap_or("");
            !ip.is_empty()
                && ip.bytes().all(|c| c.is_ascii_digit())
                && fp.bytes().all(|c| c.is_ascii_digit())
        };
        return match p[1] {
            b'%' => a.first() == Some(&b'%') && line_matches(&a[1..], rest),
            b's' => (1..=a.len()).any(|n| line_matches(&a[n..], rest)),
            b'S' => (0..=a.len()).any(|n| line_matches(&a[n..], rest)),
            b'c' => !a.is_empty() && line_matches(&a[1..], rest),
            b'd' => (1..=a.len())
                .take_while(|&n| a[n - 1].is_ascii_digit())
                .any(|n| line_matches(&a[n..], rest)),
            b'i' => {
                let off = usize::from(matches!(a.first(), Some(b'+' | b'-')));
                (off + 1..=a.len())
                    .take_while(|&n| a[n - 1].is_ascii_digit())
                    .any(|n| line_matches(&a[n..], rest))
            }
            b'f' => (1..=a.len()).any(|n| is_float(&a[..n]) && line_matches(&a[n..], rest)),
            _ => a.first() == Some(&p[0]) && line_matches(&a[1..], &p[1..]),
        };
    }
    a.first() == Some(&p[0]) && line_matches(&a[1..], &p[1..])
}

/// Asserts that `actual` matches an `EXPECTF` pattern (line by line).
#[track_caller]
fn assert_textf(actual: &str, pattern: &str) {
    let a: Vec<&str> = actual.trim_end().lines().collect();
    let p: Vec<&str> = pattern.trim_end().lines().collect();
    for (i, (la, lp)) in a.iter().zip(p.iter()).enumerate() {
        assert!(
            line_matches(la.as_bytes(), lp.as_bytes()),
            "line {} does not match\n  actual:  {la:?}\n  pattern: {lp:?}\n--- actual ---\n{actual}",
            i + 1
        );
    }
    assert_eq!(
        a.len(),
        p.len(),
        "line count differs\n--- actual ---\n{actual}\n--- pattern ---\n{pattern}"
    );
}

/// Indents every line but the first by `n` spaces (nested `var_dump()` output).
fn indent_nested(s: &str, n: usize) -> String {
    let pad = " ".repeat(n);
    let mut out = String::new();
    for (i, l) in s.trim_end().lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&pad);
        out.push_str(l);
    }
    out.push('\n');
    out
}

/// A time broken down in a zone, as `localtime()` / `getdate()` compute it.
fn local_tm(ts: i64, zone: &str) -> Time {
    let mut t = Time {
        tz_info: Some(tz(zone)),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    t.unixtime2local(ts);
    t
}

/// `localtime($ts, $associative)`.
fn localtime(ts: i64, zone: &str, assoc: bool) -> V {
    let t = local_tm(ts, zone);
    let vals = [
        ("tm_sec", t.s),
        ("tm_min", t.i),
        ("tm_hour", t.h),
        ("tm_mday", t.d),
        ("tm_mon", t.m - 1),
        ("tm_year", t.y - 1900),
        ("tm_wday", day_of_week(t.y, t.m, t.d)),
        ("tm_yday", day_of_year(t.y, t.m, t.d)),
        ("tm_isdst", i64::from(t.dst)),
    ];
    V::Arr(
        vals.iter()
            .enumerate()
            .map(|(i, (name, v))| {
                let key = if assoc { k(name) } else { K::I(i as i64) };
                (key, V::Int(*v))
            })
            .collect(),
    )
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `getdate($ts)`.
fn getdate(ts: i64, zone: &str) -> V {
    let t = local_tm(ts, zone);
    V::Arr(vec![
        (k("seconds"), V::Int(t.s)),
        (k("minutes"), V::Int(t.i)),
        (k("hours"), V::Int(t.h)),
        (k("mday"), V::Int(t.d)),
        (k("wday"), V::Int(day_of_week(t.y, t.m, t.d))),
        (k("mon"), V::Int(t.m)),
        (k("year"), V::Int(t.y)),
        (k("yday"), V::Int(day_of_year(t.y, t.m, t.d))),
        (
            k("weekday"),
            V::Str(php_timelib::format::full_day_name(t.y, t.m, t.d).to_string()),
        ),
        (
            k("month"),
            V::Str(MONTH_NAMES[(t.m - 1) as usize].to_string()),
        ),
        (K::I(0), V::Int(ts)),
    ])
}

/// `DateTimeZone::listAbbreviations()`: entries grouped by abbreviation, in order.
fn list_abbreviations() -> Vec<(&'static str, Vec<&'static TzLookupEntry>)> {
    let mut groups: Vec<(&'static str, Vec<&'static TzLookupEntry>)> = Vec::new();
    for e in timezone_abbreviations_list() {
        match groups.iter_mut().find(|g| g.0 == e.name) {
            Some(g) => g.1.push(e),
            None => groups.push((e.name, vec![e])),
        }
    }
    groups
}

/// `DatePeriod::createFromISO8601String($isostr)` / `new DatePeriod($isostr)` up to
/// the point where the ISO string is validated (`date_period_initialize` +
/// `date_period_init_iso8601_string`); the error is the exception message.
fn iso_period(isostr: &str, func: &str) -> Result<IsoInterval, String> {
    let r = strtointerval(isostr);
    if !r.errors.errors.is_empty() {
        return Err(format!("Unknown or bad format ({isostr})"));
    }
    if r.begin.is_none() {
        return Err(format!(
            "{func}(): ISO interval must contain a start date, \"{isostr}\" given"
        ));
    }
    if r.period.is_none() {
        return Err(format!(
            "{func}(): ISO interval must contain an interval, \"{isostr}\" given"
        ));
    }
    if r.end.is_none() && r.recurrences.unwrap_or(0) == 0 {
        return Err(format!(
            "{func}(): ISO interval must contain an end date or a recurrence count, \"{isostr}\" given"
        ));
    }
    Ok(r)
}

/// `date_period_advance()`: one DatePeriod iteration step.
fn period_advance(t: &mut Time, interval: &PhpInterval) {
    t.have_relative = true;
    t.relative = interval.diff;
    t.sse_uptodate = false;
    t.update_ts(None);
    t.update_from_sse();
}

/// Which part of `examine_diff()` (examine_diff.inc) to show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Diff,
    Days,
    Add,
    Sub,
}

/// `examine_diff($end, $start, $expect_spec, ...)` from examine_diff.inc.
fn examine_diff(end: &DT, start: &DT, expect_spec: &str, show: Show) -> String {
    let start_date = start.format("Y-m-d H:i:s T");
    let end_date = end.format("Y-m-d H:i:s T");
    let mut expect_interval = php::date_interval_create(&format!("P{}", &expect_spec[2..]))
        .unwrap_or_else(|e| panic!("{e}"));
    if &expect_spec[1..2] == "-" {
        expect_interval.diff.invert = true;
    }
    match show {
        Show::Diff => {
            let r = start.diff(end);
            let spec = interval_format(&r, "P%R%yY%mM%dDT%hH%iM%sS");
            format!("DIFF: {end_date} - {start_date} = **{spec}**")
        }
        Show::Days => {
            let r = start.diff(end);
            format!("DAYS: **{}**", interval_format(&r, "%a"))
        }
        Show::Add => {
            let mut s = start.clone();
            s.add_interval(&expect_interval);
            let res = s.format("Y-m-d H:i:s T");
            format!("ADD: {start_date} + {expect_spec} = **{res}**")
        }
        Show::Sub => {
            let mut e = end.clone();
            e.sub_interval(&expect_interval);
            let res = e.format("Y-m-d H:i:s T");
            format!("SUB: {end_date} - {expect_spec} = **{res}**")
        }
    }
}

const NY: &str = "America/New_York";

/// One `examine_diff()` call of the DateTime_data-*.inc files: test name, end date
/// string, whether `$end->setTimezone(new DateTimeZone('America/New_York'))` follows,
/// start date string and the expected spec.
type DiffCase = (&'static str, &'static str, bool, &'static str, &'static str);

/// DateTime_data-dates.inc
const DATES: &[DiffCase] = &[
    (
        "test__7",
        "2009-01-14",
        false,
        "2009-01-07",
        "P+0Y0M7DT0H0M0S",
    ),
    (
        "test_years_positive__7_by_0_day",
        "2007-02-07",
        false,
        "2000-02-07",
        "P+7Y0M0DT0H0M0S",
    ),
    (
        "test_years_positive__7_by_1_day",
        "2007-02-08",
        false,
        "2000-02-07",
        "P+7Y0M1DT0H0M0S",
    ),
    (
        "test_years_positive__6_shy_1_day",
        "2007-02-06",
        false,
        "2000-02-07",
        "P+6Y11M30DT0H0M0S",
    ),
    (
        "test_years_positive__7_by_1_month",
        "2007-03-07",
        false,
        "2000-02-07",
        "P+7Y1M0DT0H0M0S",
    ),
    (
        "test_years_positive__6_shy_1_month",
        "2007-01-07",
        false,
        "2000-02-07",
        "P+6Y11M0DT0H0M0S",
    ),
    (
        "test_years_positive__7_by_1_month_split_newyear",
        "2007-01-07",
        false,
        "1999-12-07",
        "P+7Y1M0DT0H0M0S",
    ),
    (
        "test_years_positive__6_shy_1_month_split_newyear",
        "2006-12-07",
        false,
        "2000-01-07",
        "P+6Y11M0DT0H0M0S",
    ),
    (
        "test_negative__7",
        "2009-01-07",
        false,
        "2009-01-14",
        "P-0Y0M7DT0H0M0S",
    ),
    (
        "test_years_negative__7_by_0_day",
        "2000-02-07",
        false,
        "2007-02-07",
        "P-7Y0M0DT0H0M0S",
    ),
    (
        "test_years_negative__7_by_1_day",
        "2000-02-07",
        false,
        "2007-02-08",
        "P-7Y0M1DT0H0M0S",
    ),
    (
        "test_years_negative__6_shy_1_day",
        "2000-02-07",
        false,
        "2007-02-06",
        "P-6Y11M28DT0H0M0S",
    ),
    (
        "test_years_negative__7_by_1_month",
        "2000-02-07",
        false,
        "2007-03-07",
        "P-7Y1M0DT0H0M0S",
    ),
    (
        "test_years_negative__6_shy_1_month",
        "2000-02-07",
        false,
        "2007-01-07",
        "P-6Y11M0DT0H0M0S",
    ),
    (
        "test_years_negative__7_by_1_month_split_newyear",
        "1999-12-07",
        false,
        "2007-01-07",
        "P-7Y1M0DT0H0M0S",
    ),
    (
        "test_years_negative__6_shy_1_month_split_newyear",
        "2000-01-07",
        false,
        "2006-12-07",
        "P-6Y11M0DT0H0M0S",
    ),
];

/// DateTime_data-spring-type2-type3.inc
const SPRING_TYPE2_TYPE3: &[DiffCase] = &[
    (
        "test_time_spring_type2_prev_type3_prev",
        "2010-03-13 18:38:28",
        false,
        "2010-02-11 02:18:48 EST",
        "P+0Y1M2DT16H19M40S",
    ),
    (
        "test_time_spring_type2_prev_type3_st",
        "2010-03-14 00:10:20",
        false,
        "2010-03-13 18:38:28 EST",
        "P+0Y0M0DT5H31M52S",
    ),
    (
        "test_time_spring_type2_prev_type3_dt",
        "2010-03-14 03:16:55",
        false,
        "2010-03-13 18:38:28 EST",
        "P+0Y0M0DT7H38M27S",
    ),
    (
        "test_time_spring_type2_prev_type3_post",
        "2010-03-15 19:59:59",
        false,
        "2010-03-13 18:38:28 EST",
        "P+0Y0M2DT1H21M31S",
    ),
    (
        "test_time_spring_type2_st_type3_prev",
        "2010-03-13 18:38:28",
        false,
        "2010-03-14 00:10:20 EST",
        "P-0Y0M0DT5H31M52S",
    ),
    (
        "test_time_spring_type2_st_type3_st",
        "2010-03-14 00:15:35",
        false,
        "2010-03-14 00:10:20 EST",
        "P+0Y0M0DT0H5M15S",
    ),
    (
        "test_time_spring_type2_st_type3_dt",
        "2010-03-14 03:16:55",
        false,
        "2010-03-14 00:10:20 EST",
        "P+0Y0M0DT2H6M35S",
    ),
    (
        "test_time_spring_type2_st_type3_post",
        "2010-03-15 19:59:59",
        false,
        "2010-03-14 00:10:20 EST",
        "P+0Y0M1DT18H49M39S",
    ),
    (
        "test_time_spring_type2_dt_type3_prev",
        "2010-03-13 18:38:28",
        false,
        "2010-03-14 03:16:55 EDT",
        "P-0Y0M0DT7H38M27S",
    ),
    (
        "test_time_spring_type2_dt_type3_st",
        "2010-03-14 00:10:20",
        false,
        "2010-03-14 03:16:55 EDT",
        "P-0Y0M0DT2H6M35S",
    ),
    (
        "test_time_spring_type2_dt_type3_dt",
        "2010-03-14 05:19:56",
        false,
        "2010-03-14 03:16:55 EDT",
        "P+0Y0M0DT2H3M1S",
    ),
    (
        "test_time_spring_type2_dt_type3_post",
        "2010-03-15 19:59:59",
        false,
        "2010-03-14 03:16:55 EDT",
        "P+0Y0M1DT16H43M4S",
    ),
    (
        "test_time_spring_type2_post_type3_prev",
        "2010-03-13 18:38:28",
        false,
        "2010-03-15 19:59:59 EDT",
        "P-0Y0M2DT1H21M31S",
    ),
    (
        "test_time_spring_type2_post_type3_st",
        "2010-03-14 00:10:20",
        false,
        "2010-03-15 19:59:59 EDT",
        "P-0Y0M1DT18H49M39S",
    ),
    (
        "test_time_spring_type2_post_type3_dt",
        "2010-03-14 03:16:55",
        false,
        "2010-03-15 19:59:59 EDT",
        "P-0Y0M1DT16H43M4S",
    ),
    (
        "test_time_spring_type2_post_type3_post",
        "2010-03-15 19:59:59",
        false,
        "2010-03-15 18:57:55 EDT",
        "P+0Y0M0DT1H2M4S",
    ),
    (
        "test_time_spring_type2_stsec_type3_dtsec",
        "2010-03-14 03:00:00",
        false,
        "2010-03-14 01:59:59 EST",
        "P+0Y0M0DT0H0M1S",
    ),
    (
        "test_time_spring_type2_dtsec_type3_stsec",
        "2010-03-14 01:59:59",
        false,
        "2010-03-14 03:00:00 EDT",
        "P-0Y0M0DT0H0M1S",
    ),
];

/// DateTime_data-fall-type2-type3.inc
const FALL_TYPE2_TYPE3: &[DiffCase] = &[
    (
        "test_time_fall_type2_prev_type3_prev",
        "2010-11-06 18:38:28",
        false,
        "2010-10-04 02:18:48 EDT",
        "P+0Y1M2DT16H19M40S",
    ),
    (
        "test_time_fall_type2_prev_type3_dt",
        "2010-11-07 00:10:20",
        false,
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT5H31M52S",
    ),
    (
        "test_time_fall_type2_prev_type3_redodt",
        "2010-11-07 01:12:33 EDT",
        true,
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT6H34M5S",
    ),
    (
        "test_time_fall_type2_prev_type3_redost",
        "2010-11-07 01:14:44 EST",
        true,
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT7H36M16S",
    ),
    (
        "test_time_fall_type2_prev_type3_st",
        "2010-11-07 03:16:55",
        false,
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT9H38M27S",
    ),
    (
        "test_time_fall_type2_prev_type3_post",
        "2010-11-08 19:59:59",
        false,
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M2DT1H21M31S",
    ),
    (
        "test_time_fall_type2_dt_type3_prev",
        "2010-11-06 18:38:28",
        false,
        "2010-11-07 00:10:20 EDT",
        "P-0Y0M0DT5H31M52S",
    ),
    (
        "test_time_fall_type2_dt_type3_dt",
        "2010-11-07 00:15:35",
        false,
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT0H5M15S",
    ),
    (
        "test_time_fall_type2_dt_type3_redodt",
        "2010-11-07 01:12:33 EDT",
        true,
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT1H2M13S",
    ),
    (
        "test_time_fall_type2_dt_type3_redost",
        "2010-11-07 01:14:44 EST",
        true,
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT2H4M24S",
    ),
    (
        "test_time_fall_type2_dt_type3_st",
        "2010-11-07 03:16:55",
        false,
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT4H6M35S",
    ),
    (
        "test_time_fall_type2_dt_type3_post",
        "2010-11-08 19:59:59",
        false,
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M1DT20H49M39S",
    ),
    (
        "test_time_fall_type2_redodt_type3_prev",
        "2010-11-06 18:38:28",
        false,
        "2010-11-07 01:12:33 EDT",
        "P-0Y0M0DT6H34M5S",
    ),
    (
        "test_time_fall_type2_redodt_type3_dt",
        "2010-11-07 00:10:20",
        false,
        "2010-11-07 01:12:33 EDT",
        "P-0Y0M0DT1H2M13S",
    ),
    (
        "test_time_fall_type2_redodt_type3_redodt",
        "2010-11-07 01:15:35 EDT",
        true,
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M0DT0H3M2S",
    ),
    (
        "test_time_fall_type2_redodt_type3_redost",
        "2010-11-07 01:14:44 EST",
        true,
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M0DT1H2M11S",
    ),
    (
        "test_time_fall_type2_redodt_type3_st",
        "2010-11-07 03:16:55",
        false,
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M0DT3H4M22S",
    ),
    (
        "test_time_fall_type2_redodt_type3_post",
        "2010-11-08 19:59:59",
        false,
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M1DT19H47M26S",
    ),
    (
        "test_time_fall_type2_redost_type3_prev",
        "2010-11-06 18:38:28",
        false,
        "2010-11-07 01:14:44 EST",
        "P-0Y0M0DT7H36M16S",
    ),
    (
        "test_time_fall_type2_redost_type3_dt",
        "2010-11-07 00:10:20",
        false,
        "2010-11-07 01:14:44 EST",
        "P-0Y0M0DT2H4M24S",
    ),
    (
        "test_time_fall_type2_redost_type3_redodt",
        "2010-11-07 01:12:33 EDT",
        true,
        "2010-11-07 01:14:44 EST",
        "P-0Y0M0DT1H2M11S",
    ),
    (
        "test_time_fall_type2_redost_type3_redost",
        "2010-11-07 01:16:54 EST",
        true,
        "2010-11-07 01:14:44 EST",
        "P+0Y0M0DT0H2M10S",
    ),
    (
        "test_time_fall_type2_redost_type3_st",
        "2010-11-07 03:16:55",
        false,
        "2010-11-07 01:14:44 EST",
        "P+0Y0M0DT2H2M11S",
    ),
    (
        "test_time_fall_type2_redost_type3_post",
        "2010-11-08 19:59:59",
        false,
        "2010-11-07 01:14:44 EST",
        "P+0Y0M1DT18H45M15S",
    ),
    (
        "test_time_fall_type2_st_type3_prev",
        "2010-11-06 18:38:28",
        false,
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT9H38M27S",
    ),
    (
        "test_time_fall_type2_st_type3_dt",
        "2010-11-07 00:10:20",
        false,
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT4H6M35S",
    ),
    (
        "test_time_fall_type2_st_type3_redodt",
        "2010-11-07 01:12:33 EDT",
        true,
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT3H4M22S",
    ),
    (
        "test_time_fall_type2_st_type3_redost",
        "2010-11-07 01:14:44 EST",
        true,
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT2H2M11S",
    ),
    (
        "test_time_fall_type2_st_type3_st",
        "2010-11-07 05:19:56",
        false,
        "2010-11-07 03:16:55 EST",
        "P+0Y0M0DT2H3M1S",
    ),
    (
        "test_time_fall_type2_st_type3_post",
        "2010-11-08 19:59:59",
        false,
        "2010-11-07 03:16:55 EST",
        "P+0Y0M1DT16H43M4S",
    ),
    (
        "test_time_fall_type2_post_type3_prev",
        "2010-11-06 18:38:28",
        false,
        "2010-11-08 19:59:59 EST",
        "P-0Y0M2DT1H21M31S",
    ),
    (
        "test_time_fall_type2_post_type3_dt",
        "2010-11-07 00:10:20",
        false,
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT20H49M39S",
    ),
    (
        "test_time_fall_type2_post_type3_redodt",
        "2010-11-07 01:12:33 EDT",
        true,
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT19H47M26S",
    ),
    (
        "test_time_fall_type2_post_type3_redost",
        "2010-11-07 01:14:44 EST",
        true,
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT18H45M15S",
    ),
    (
        "test_time_fall_type2_post_type3_st",
        "2010-11-07 03:16:55",
        false,
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT16H43M4S",
    ),
    (
        "test_time_fall_type2_post_type3_post",
        "2010-11-08 19:59:59",
        false,
        "2010-11-08 18:57:55 EST",
        "P+0Y0M0DT1H2M4S",
    ),
    (
        "test_time_fall_type2_dtsec_type3_stsec",
        "2010-11-07 01:00:00 EST",
        true,
        "2010-11-07 01:59:59 EDT",
        "P+0Y0M0DT0H0M1S",
    ),
    (
        "test_time_fall_type2_stsec_type3_dtsec",
        "2010-11-07 01:59:59 EDT",
        true,
        "2010-11-07 01:00:00 EST",
        "P-0Y0M0DT0H0M1S",
    ),
];

/// Runs a DateTime_data-*.inc file through `examine_diff()`.
fn run_diff_cases(cases: &[DiffCase], show: Show) -> String {
    let mut out = String::new();
    for &(name, end_s, end_settz, start_s, spec) in cases {
        let mut end = DT::new(end_s, NY);
        if end_settz {
            end.set_timezone(NY);
        }
        let start = DT::new(start_s, NY);
        out.push_str(&format!(
            "{name}: {}\n",
            examine_diff(&end, &start, spec, show)
        ));
    }
    out
}

/// DateTime_data-massive.inc
fn run_massive(show: Show) -> String {
    let make = |y: i64| {
        let mut d = DT::new("now", NY);
        d.set_date(y, 1, 1);
        d.set_time(16, 18, 2, 0);
        d
    };
    let mut out = String::new();
    let (end, start) = (make(333_333), make(-333_333));
    let line = examine_diff(&end, &start, "P+666666Y0M0DT0H0M0S", show);
    out.push_str(&format!("test_massive_positive: {line}\n"));
    let (end, start) = (make(-333_333), make(333_333));
    let line = examine_diff(&end, &start, "P-666666Y0M0DT0H0M0S", show);
    out.push_str(&format!("test_massive_negative: {line}\n"));
    out
}

// ---------------------------------------------------------------------------------
// Tests

/// Sanity check of the `EXPECTF` matcher used below.
#[test]
fn expectf_matcher_self_check() {
    let m = |a: &str, p: &str| line_matches(a.as_bytes(), p.as_bytes());
    assert!(m("int(2005)", "int(%d)"));
    assert!(!m("int()", "int(%d)"));
    assert!(!m("int(20a5)", "int(%d)"));
    assert!(m("-0500", "-0%d"));
    assert!(m("x EST y", "x %s y"));
    assert!(!m("x  y", "x %s y"));
    assert!(m("[comments] => ", "[comments] => %S"));
    assert!(!m("[comments] => ", "[comments] => %s"));
    assert!(m("[latitude] => -33.86667", "[latitude] => %f"));
    assert!(m("[latitude] => 34", "[latitude] => %f"));
    assert!(!m("[latitude] => abc", "[latitude] => %f"));
    assert!(!m("abc", "abd"));
    assert!(!m("abc", "ab"));
}

/// ext/date/tests/007.phpt (`localtime()` without a timestamp uses the fixed
/// [`NOW`]; its output is `%d` only).
#[test]
fn phpt_007() {
    let t = mktime(0, 0, 0, 6, 27, 2006, "UTC");
    let mut out = String::new();
    out.push_str(&localtime(NOW, "UTC", false).var_dump());
    out.push_str(&localtime(t, "UTC", false).var_dump());
    out.push_str(&localtime(t, "UTC", true).var_dump());
    out.push_str(&localtime(t, "UTC", false).var_dump());
    out.push_str("Done\n");
    assert_textf(&out, EXP_007);
}

/// ext/date/tests/012.phpt
#[test]
fn phpt_012() {
    let mut dto = DT::new("2006-12-12", "UTC");
    let mut out = String::new();
    for (y, w, d) in [(2006, 2, 15), (2006, 5, 1), (2006, 100, 15)] {
        dto.set_isodate(y, w, d);
        out.push_str(&dto.var_dump("DateTime", 1));
        out.push_str(&vstr(&dto.format("Y/m/d H:i:s")));
    }
    out.push_str("Done\n");
    assert_text(&out, EXP_012);
}

/// ext/date/tests/DateInterval_createFromDateString_broken.phpt
#[test]
fn dateinterval_createfromdatestring_broken() {
    assert!(php::date_interval_create_from_date_string("foobar").is_none());
    let r = php_timelib::strtotime("foobar", &builtin_tzinfo);
    let e = &r.errors.errors[0];
    let ch = if e.character == 0 {
        ' '
    } else {
        e.character as char
    };
    let msg = format!(
        "DateMalformedIntervalStringException: Unknown or bad format ({}) at position {} ({}): {}",
        "foobar", e.position, ch, e.message
    );
    assert_eq!(
        msg,
        "DateMalformedIntervalStringException: Unknown or bad format (foobar) at position 0 (f): The timezone could not be found in the database"
    );
}

/// ext/date/tests/DateInterval_serialize-003.phpt (partial: only the "Used
/// serialised interval" part; the (un)serialised interval re-parses "next weekday").
#[test]
fn dateinterval_serialize_003() {
    let e = php::date_interval_create_from_date_string("next weekday").unwrap();
    let now = DT::new("2022-04-22 16:25:11 BST", "Europe/London");
    let mut added = now.clone();
    added.add_interval(&e);
    assert_text(
        &added.var_dump("DateTimeImmutable", 4),
        r#"object(DateTimeImmutable)#4 (3) {
  ["date"]=>
  string(26) "2022-04-25 16:25:11.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "BST"
}"#,
    );
    // DateInvalidOperationException: DateTimeImmutable::sub(): Only non-special
    // relative time specifications are supported for subtraction
    assert!(php::date_sub(&now.t, &e).is_none());
}

/// ext/date/tests/DatePeriod_createFromISO8601String_static_return.phpt (the
/// DatePeriod object dump is assembled from its parts)
#[test]
fn dateperiod_createfromiso8601string_static_return() {
    let func = "DatePeriod::createFromISO8601String";
    let r = iso_period("R4/2012-07-01T00:00:00Z/P7D", func).unwrap();
    let mut start = r.begin.unwrap();
    start.update_ts(None);
    let start = DT {
        t: start,
        default_zone: "UTC".into(),
    };
    let interval = PhpInterval {
        diff: r.period.unwrap(),
        wall: true,
    };
    // recurrences + include_start_date + include_end_date
    let recurrences = i64::from(r.recurrences.unwrap_or(0)) + 1;
    let mut out = String::from("object(MyDatePeriod)#1 (7) {\n");
    out.push_str("  [\"start\"]=>\n");
    out.push_str(&indent_nested(&start.var_dump("DateTimeImmutable", 2), 2));
    out.push_str("  [\"current\"]=>\n  NULL\n  [\"end\"]=>\n  NULL\n");
    out.push_str("  [\"interval\"]=>\n");
    out.push_str(&indent_nested(&interval_var_dump(&interval, 3), 2));
    out.push_str(&format!("  [\"recurrences\"]=>\n  int({recurrences})\n"));
    out.push_str("  [\"include_start_date\"]=>\n  bool(true)\n");
    out.push_str("  [\"include_end_date\"]=>\n  bool(false)\n}\n");
    for s in ["R4/2012-07-01T00:/P7D", "R4/2012-07-01T00:00:00Z"] {
        let e = iso_period(s, func).unwrap_err();
        out.push_str(&format!("DateMalformedPeriodStringException: {e}\n"));
    }
    assert_text(&out, EXP_DATEPERIOD_CREATEFROMISO8601STRING_STATIC_RETURN);
}

/// ext/date/tests/DateTimeImmutable_createFromInterface-001.phpt
#[test]
fn datetimeimmutable_createfrominterface_001() {
    let mut out = String::new();
    for s in [
        "2014-03-02 16:24:08",
        "2019-12-16 15:06:46 CET",
        "2019-12-16 15:08:20 +0100",
    ] {
        // date_create() and date_create_immutable(); createFromInterface() clones.
        for _ in 0..2 {
            out.push_str(&DT::new(s, "Europe/London").var_dump("DateTimeImmutable", 1));
        }
    }
    assert_textf(&out, EXP_DATETIMEIMMUTABLE_CREATEFROMINTERFACE_001);
}

/// ext/date/tests/DateTimeZone_getLocation.phpt
///
/// The military-zone entries ("a", "b", ...) of the abbreviation table have a NULL
/// `timezone_id`, which the script skips.
#[test]
fn datetimezone_getlocation() {
    let mut country_code = vec!["??".to_string()];
    let country_code_test = ["AU", "CA", "ET", "AF", "US", "KZ", "AM"];
    let mut out = String::new();
    for (_, value) in list_abbreviations() {
        let Some(id) = value[0].full_tz_name else {
            continue;
        };
        let zone = php::timezone_open(id).unwrap_or_else(|e| panic!("{e}"));
        let PhpTimeZone::Id(tzi) = zone else {
            continue; // getLocation() returns false
        };
        let loc = &tzi.location;
        if !country_code.contains(&loc.country_code) && !loc.country_code.is_empty() {
            country_code.push(loc.country_code.clone());
            if country_code_test.contains(&loc.country_code.as_str()) {
                let a = V::Arr(vec![
                    (k("country_code"), V::Str(loc.country_code.clone())),
                    (k("latitude"), V::Float(loc.latitude)),
                    (k("longitude"), V::Float(loc.longitude)),
                    (k("comments"), V::Str(loc.comments.clone())),
                ]);
                out.push_str(&a.print_r());
            }
        }
    }
    assert_textf(&out, EXP_DATETIMEZONE_GETLOCATION);
}

/// ext/date/tests/DateTimeZone_listAbbreviations_basic1.phpt
#[test]
fn datetimezone_listabbreviations_basic1() {
    let abbr = list_abbreviations();
    let mut out =
        String::from("*** Testing DateTimeZone::listAbbreviations() : basic functionality ***\n");
    out.push_str(&vstr("array"));
    out.push_str(&V::Int(abbr.len() as i64).var_dump());
    out.push_str("\n-- Format a sample entry --\n");
    let acst = &abbr.iter().find(|g| g.0 == "acst").unwrap().1;
    let v = V::Arr(
        acst.iter()
            .enumerate()
            .map(|(i, e)| {
                (
                    K::I(i as i64),
                    V::Arr(vec![
                        (k("dst"), V::Bool(e.dst)),
                        (k("offset"), V::Int(i64::from(e.gmtoffset))),
                        (
                            k("timezone_id"),
                            e.full_tz_name.map_or(V::Null, |id| V::Str(id.to_string())),
                        ),
                    ]),
                )
            })
            .collect(),
    );
    out.push_str(&v.var_dump());
    assert_text(&out, EXP_DATETIMEZONE_LISTABBREVIATIONS_BASIC1);
}

/// ext/date/tests/DateTimeZone_serialize_type_3.phpt (partial: no serialisation)
#[test]
fn datetimezone_serialize_type_3() {
    let z = php::timezone_open("America/New_York").unwrap();
    let obj = V::Obj(
        "DateTimeZone".into(),
        1,
        vec![
            (
                k("timezone_type"),
                V::Int(i64::from(z.zone_type().as_i32())),
            ),
            (k("timezone"), V::Str(z.name())),
        ],
    );
    assert_text(
        &obj.var_dump(),
        r#"object(DateTimeZone)#1 (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(16) "America/New_York"
}"#,
    );
    assert_eq!(vstr(&z.name()), "string(16) \"America/New_York\"\n");
}

/// ext/date/tests/DateTime_add-fall-type2-type3.phpt
#[test]
fn datetime_add_fall_type2_type3() {
    assert_text(
        &run_diff_cases(FALL_TYPE2_TYPE3, Show::Add),
        EXP_DATETIME_ADD_FALL_TYPE2_TYPE3,
    );
}

/// ext/date/tests/DateTime_add-spring-type2-type3.phpt
#[test]
fn datetime_add_spring_type2_type3() {
    assert_text(
        &run_diff_cases(SPRING_TYPE2_TYPE3, Show::Add),
        EXP_DATETIME_ADD_SPRING_TYPE2_TYPE3,
    );
}

/// ext/date/tests/DateTime_clone_basic4.phpt (partial: the `__clone` echo is PHP
/// object semantics)
#[test]
fn datetime_clone_basic4() {
    let d1 = DT::new("2009-02-03 12:34:41 GMT", "Europe/London");
    let d1_clone = d1.clone();
    assert_eq!(vstr(&d1_clone.format("m.d.y")), "string(8) \"02.03.09\"\n");
}

/// ext/date/tests/DateTime_days-dates.phpt
#[test]
fn datetime_days_dates() {
    assert_text(&run_diff_cases(DATES, Show::Days), EXP_DATETIME_DAYS_DATES);
}

/// ext/date/tests/DateTime_days-massive.phpt
#[test]
fn datetime_days_massive() {
    assert_text(&run_massive(Show::Days), EXP_DATETIME_DAYS_MASSIVE);
}

/// ext/date/tests/DateTime_diff-dates.phpt
#[test]
fn datetime_diff_dates() {
    assert_text(&run_diff_cases(DATES, Show::Diff), EXP_DATETIME_DIFF_DATES);
}

/// ext/date/tests/DateTime_diff-massive.phpt
#[test]
fn datetime_diff_massive() {
    assert_textf(&run_massive(Show::Diff), EXP_DATETIME_DIFF_MASSIVE);
}

/// ext/date/tests/DateTime_extends_basic2.phpt (partial: the subclass only passes
/// the default timezone explicitly)
#[test]
fn datetime_extends_basic2() {
    let d = DT::new_in("1967-05-01 22:30:41", "Europe/London", "Europe/London");
    assert_eq!(d.format("F j, Y, g:i:s a"), "May 1, 1967, 10:30:41 pm");
}

/// ext/date/tests/DateTime_getTimeZone_basic1.phpt
#[test]
fn datetime_gettimezone_basic1() {
    let object = DT::new("2009-01-30 17:57:32", "Europe/London");
    assert_eq!(object.timezone_name(), "Europe/London");
    let mut object = DT::new("2009-01-30 17:57:32", "America/New_York");
    assert_eq!(object.timezone_name(), "America/New_York");
    object.set_timezone("America/Los_Angeles");
    assert_eq!(object.timezone_name(), "America/Los_Angeles");
}

/// ext/date/tests/DateTime_setDate_basic1.phpt
#[test]
fn datetime_setdate_basic1() {
    let mut datetime = DT::new("2009-01-30 19:34:10", "Europe/London");
    assert_eq!(
        datetime.format("D, d M Y H:i:s O"),
        "Fri, 30 Jan 2009 19:34:10 +0000"
    );
    datetime.set_date(2008, 2, 1);
    assert_eq!(
        datetime.format("D, d M Y H:i:s O"),
        "Fri, 01 Feb 2008 19:34:10 +0000"
    );
}

/// ext/date/tests/DateTime_sub-dates.phpt
#[test]
fn datetime_sub_dates() {
    assert_text(&run_diff_cases(DATES, Show::Sub), EXP_DATETIME_SUB_DATES);
}

/// ext/date/tests/DateTime_sub-massive.phpt
#[test]
fn datetime_sub_massive() {
    assert_textf(&run_massive(Show::Sub), EXP_DATETIME_SUB_MASSIVE);
}

/// ext/date/tests/bug14561.phpt
#[test]
fn bug14561() {
    assert_eq!(
        strtotime_abs("19:30 Dec 17 2005", "GMT"),
        Some(1_134_847_800)
    );
    assert_eq!(
        strtotime_abs("Dec 17 19:30 2005", "GMT"),
        Some(1_134_847_800)
    );
}

/// ext/date/tests/bug26090.phpt
#[test]
fn bug26090() {
    for t in ["2003-10-28 10:20:30-0800", "2003-10-28 10:20:30-08:00"] {
        let ts = strtotime_abs(t, NY).unwrap();
        assert_eq!(date("Y-m-d H:i:s T", ts, NY), "2003-10-28 13:20:30 EST");
    }
}

/// ext/date/tests/bug28024.phpt
#[test]
fn bug28024() {
    let ts = strtotime_abs("17:00 2004-01-01", "Europe/Berlin").unwrap();
    assert_eq!(ts, 1_072_972_800);
    assert_eq!(
        date("Y-m-d H:i:s T", ts, "Europe/Berlin"),
        "2004-01-01 17:00:00 CET"
    );
}

/// ext/date/tests/bug30096.phpt
#[test]
fn bug30096() {
    fn gm_date_check(
        out: &mut String,
        ts: &mut i64,
        (h, i, s, m, d, y): (i64, i64, i64, i64, i64, i64),
    ) {
        out.push_str(&format!("gmmktime({h},{i},{s},{m},{d},{y}): "));
        let tsold = *ts;
        *ts = gmmktime(h, i, s, m, d, y);
        out.push_str(&format!(
            "{} | gmdate('r', {}):{}",
            *ts,
            *ts,
            gmdate("r", *ts)
        ));
        if tsold > 0 {
            out.push_str(&format!(" | Diff: {}", *ts - tsold));
        }
        out.push('\n');
    }
    let mut out = String::from("no dst --> dst\n");
    let mut ts: i64 = -1;
    for h in 1..=4 {
        gm_date_check(&mut out, &mut ts, (h, 0, 0, 3, 27, 2005));
    }
    out.push_str("\ndst --> no dst\n");
    ts = -1;
    for h in 1..=4 {
        gm_date_check(&mut out, &mut ts, (h, 0, 0, 10, 30, 2005));
    }
    assert_text(&out, EXP_BUG30096);
}

/// ext/date/tests/bug33056.phpt
#[test]
fn bug33056() {
    let mut out = String::new();
    for s in ["20050518t090000Z", "20050518t091234Z", "20050518t191234Z"] {
        out.push_str(&format!("{}\n", strtotime_abs(s, "GMT").unwrap()));
    }
    assert_text(&out, EXP_BUG33056);
}

/// ext/date/tests/bug33532.phpt (partial: the `strftime()` lines are skipped)
#[test]
fn bug33532() {
    let input = "10:00:00 AM July 1 2005";
    let ts = strtotime_abs(input, "UTC").unwrap();
    assert_eq!(
        date("H:i:s A F j Y T", ts, "UTC"),
        "10:00:00 AM July 1 2005 UTC"
    );
    let ts = strtotime_abs(input, "Australia/Sydney").unwrap();
    assert_eq!(
        date("H:i:s A F j Y T", ts, "Australia/Sydney"),
        "10:00:00 AM July 1 2005 AEST"
    );
}

/// ext/date/tests/bug33957.phpt
#[test]
fn bug33957() {
    let z = "UTC";
    let w = |s: &str| date("W", strtotime_abs(s, z).unwrap(), z);
    let o = |s: &str| date("o", strtotime_abs(s, z).unwrap(), z);
    let num = |s: String| s.parse::<i64>().unwrap();
    let mut out = String::new();
    for i in 14..=31 {
        out.push_str(&format!("1992-12-{i}  {}\n", w(&format!("1992-12-{i}"))));
    }
    for i in 1..=8 {
        out.push_str(&format!("1993-01-{i}  {}\n", w(&format!("1993-01-{i}"))));
    }
    out.push_str("----\n");
    out.push_str("             ");
    for year in 1992..=2019 {
        out.push_str(&format!("{year}     "));
    }
    out.push('\n');
    for i in 14..=31 {
        out.push_str(&format!("   (12-{i}) "));
        for year in 1992..=2019 {
            let s = format!("{year}-12-{i}");
            out.push_str(&format!(" {:02}-", num(w(&s))));
            out.push_str(&format!("{:04} ", num(o(&s))));
        }
        out.push('\n');
    }
    for i in 1..=18 {
        out.push_str(&format!("+1 (01-{i:02}) "));
        for year in 1993..=2020 {
            let s = format!("{year}-1-{i}");
            out.push_str(&format!(" {:02}-", num(w(&s))));
            out.push_str(&format!("{:04} ", num(o(&s))));
        }
        out.push('\n');
    }
    out.push_str("----\n");
    assert_text(&out, EXP_BUG33957);
}

/// ext/date/tests/bug35218.phpt
#[test]
fn bug35218() {
    assert_eq!(
        strtotime_abs("Sun, 13 Nov 2005 22:56:10 -0800 (PST)", "UTC"),
        Some(1_131_951_370)
    );
    assert_eq!(
        strtotime_abs("Sun, 13 Nov 2005 22:56:10 -0800", "UTC"),
        Some(1_131_951_370)
    );
}

/// ext/date/tests/bug35624.phpt (relative to the fixed [`NOW`]; the weekday name
/// does not depend on it)
#[test]
fn bug35624() {
    let days = [
        "monday",
        "mon",
        "tuesday",
        "tue",
        "wednesday",
        "wed",
        "thursday",
        "thu",
        "friday",
        "fri",
        "saturday",
        "sat",
        "sunday",
        "sun",
    ];
    let mut out = String::new();
    for day in days {
        let ucfirst = format!("{}{}", day[..1].to_uppercase(), &day[1..]);
        out.push_str(&date("D", strtotime_abs(day, "UTC").unwrap(), "UTC"));
        out.push_str(&date("D", strtotime_abs(&ucfirst, "UTC").unwrap(), "UTC"));
        out.push('\n');
    }
    assert_text(&out, EXP_BUG35624);
}

/// ext/date/tests/bug36224.phpt
#[test]
fn bug36224() {
    let z = "Europe/Oslo";
    let atom = "Y-m-d\\TH:i:sP";
    let ts = strtotime_abs("2006-01-31T19:23:56Z", z).unwrap();
    assert_eq!(date(atom, ts, z), "2006-01-31T20:23:56+01:00");
    let ts = strtotime_abs("2006-01-31T19:23:56", z).unwrap();
    assert_eq!(date(atom, ts, z), "2006-01-31T19:23:56+01:00");
}

/// ext/date/tests/bug37514.phpt
#[test]
fn bug37514() {
    let mut out = String::new();
    for s in [
        "May 18th 5:05",
        "May 18th 5:05pm",
        "May 18th 5:05 pm",
        "May 18th 5:05am",
        "May 18th 5:05 am",
        "May 18th 2006 5:05pm",
    ] {
        let ts = strtotime(s, 1_168_156_376, "UTC").unwrap();
        out.push_str(&date("r", ts, "UTC"));
        out.push('\n');
    }
    assert_text(&out, EXP_BUG37514);
}

/// ext/date/tests/bug40861.phpt
#[test]
fn bug40861() {
    let z = "GMT";
    let ts = strtotime_abs("2000-01-01 12:00:00", z).unwrap();
    let f = |s: &str| date("Y-m-d H:i:s", strtotime(s, ts, z).unwrap(), z);
    // "+$offset minutes" / "-$offset minutes" with $offset = +60, -60, -60, 60
    assert_eq!(f("+60 minutes"), "2000-01-01 13:00:00");
    assert_eq!(f("+-60 minutes"), "2000-01-01 11:00:00");
    assert_eq!(f("--60 minutes"), "2000-01-01 13:00:00");
    assert_eq!(f("+60 minutes"), "2000-01-01 13:00:00");
}

/// ext/date/tests/bug41844.phpt
#[test]
fn bug41844() {
    let mut out = String::new();
    for m in ["-3006 years", "-2008 years"] {
        let mut date = DT::new("2007-06-28", "UTC");
        assert!(date.modify(m));
        out.push_str(&date.format("Y-m-d\\TH:i:sO"));
        out.push('\n');
    }
    assert_text(&out, EXP_BUG41844);
}

/// ext/date/tests/bug43527.phpt
#[test]
fn bug43527() {
    let datetime = DT::new("Fri, 07 Dec 2007 19:05:14 +1000", "Etc/GMT+1");
    assert_eq!(datetime.timezone_name(), "+10:00");
}

/// ext/date/tests/bug45081.phpt
#[test]
fn bug45081() {
    let z = "Asia/Singapore";
    assert_eq!(
        strtotime_abs("2008-05-23 00:00:00 +08", z),
        Some(1_211_472_000)
    );
    assert_eq!(strtotime_abs("2008-05-23 00:00:00", z), Some(1_211_472_000));
}

/// ext/date/tests/bug48276.phpt
#[test]
fn bug48276() {
    assert_eq!(
        vstr(&date("Y", 1_245_623_227, "UTC")),
        "string(4) \"2009\"\n"
    );
}

/// ext/date/tests/bug50680.phpt
#[test]
fn bug50680() {
    let ts = strtotime_abs("March 1 eighth day 2009", "UTC").unwrap();
    assert_eq!(date("d", ts, "UTC"), "09");
}

/// ext/date/tests/bug51987.phpt
#[test]
fn bug51987() {
    let d2 = DT::new("1985-102", "Europe/London");
    assert_textf(&d2.var_dump("DateTime", 1), EXP_BUG51987);
}

/// ext/date/tests/bug52290.phpt
#[test]
fn bug52290() {
    let z = "UTC";
    let f = "o-\\WW-N | Y-m-d | H:i:s | U";
    let ts = strtotime_abs("2006-01-01", z).unwrap();
    let mut dt = DT::new(&format!("@{ts}"), z);
    dt.set_timezone(z);
    let mut out = vstr(&dt.format(f));
    dt.set_isodate(2005, 52, 1);
    out.push_str(&vstr(&dt.format(f)));
    dt.set_date(2007, 10, 10);
    out.push_str(&vstr(&dt.format(f)));
    dt.set_time(20, 30, 40, 0);
    out.push_str(&vstr(&dt.format(f)));
    assert_text(&out, EXP_BUG52290);
}

/// ext/date/tests/bug52668.phpt (DatePeriod iteration via `date_period_advance()`)
#[test]
fn bug52668() {
    let start = DT::new("20101212", "UTC");
    let interval = php::date_interval_create_from_date_string("next day").unwrap();
    // recurrences = 1 + include_start_date
    let recurrences = 2;
    let mut out = String::new();
    for _ in 0..2 {
        let mut current = start.clone();
        for idx in 0..recurrences {
            if idx > 0 {
                period_advance(&mut current.t, &interval);
            }
            out.push_str(&current.format("r"));
            out.push('\n');
        }
        out.push_str(&start.format("r"));
        out.push('\n');
    }
    assert_text(&out, EXP_BUG52668);
}

/// ext/date/tests/bug54283.phpt (partial: `new DatePeriod(NULL)` parses "")
#[test]
fn bug54283() {
    let e = iso_period("", "DatePeriod::__construct").unwrap_err();
    assert_eq!(
        format!("DateMalformedPeriodStringException: {e}"),
        "DateMalformedPeriodStringException: Unknown or bad format ()"
    );
}

/// ext/date/tests/bug61642.phpt
#[test]
fn bug61642() {
    let dates = [
        "2012-03-29",
        "2012-03-30",
        "2012-03-31",
        "2012-04-01",
        "2012-04-02",
        "2012-04-03",
        "2012-04-04",
        "2012-04-05",
    ];
    let header: Vec<String> = dates
        .iter()
        .map(|d| DT::new(d, "UTC").format("Y-m-d D"))
        .collect();
    let mut out = format!("###  {}\n\n", header.join("  "));
    for days in -11..=11 {
        out.push_str(&format!("{days:+3}  "));
        let line: Vec<String> = dates
            .iter()
            .map(|d| {
                let mut date = DT::new(d, "UTC");
                assert!(date.modify(&format!("{days} weekdays")));
                date.format("Y-m-d D")
            })
            .collect();
        out.push_str(&line.join("  "));
        out.push('\n');
    }
    assert_text(&out, EXP_BUG61642);
}

/// ext/date/tests/bug64653.phpt
#[test]
fn bug64653() {
    let mut date = DT::new("2370-01-31", "UTC");
    date.sub("P1M");
    assert_eq!(date.format("Y-m-d"), "2369-12-31");
}

/// ext/date/tests/bug65502.phpt (partial: the class of the result is not
/// expressible; only that createFromFormat() succeeds)
#[test]
fn bug65502() {
    assert!(DT::from_format("j-M-Y", "12-Sep-2013", None, "UTC").is_some());
}

/// ext/date/tests/bug66836.phpt
#[test]
fn bug66836() {
    for timestamp in ["-1", "-86400", "-1000000"] {
        let dt = DT::from_format("U", timestamp, None, "Europe/Berlin").unwrap();
        assert_eq!(dt.format("U"), timestamp);
    }
}

/// ext/date/tests/bug67253.phpt
#[test]
fn bug67253() {
    let z = "";
    let s = format!("0=G{z}9UCNnF");
    assert_text(&date_parse_from_format("aHa0", &s).var_dump(), EXP_BUG67253);
}

/// ext/date/tests/bug68549-dst-transition-DateTime-setTimestamp.phpt
#[test]
fn bug68549_dst_transition_datetime_settimestamp() {
    let zones: [(&str, &[i64]); 4] = [
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
    let mut out = String::new();
    for (timezone, timestamps) in zones {
        for &timestamp in timestamps {
            let mut dt = DT::new_in("now", timezone, "UTC");
            dt.set_timestamp(timestamp);
            out.push_str(&format!(
                "{timezone} {timestamp} -> {} {}\n",
                dt.timestamp(),
                dt.format("Y-m-d\\TH:i:sO")
            ));
        }
    }
    assert_text(&out, EXP_BUG68549_DST_TRANSITION_DATETIME_SETTIMESTAMP);
}

/// ext/date/tests/bug69336.phpt (the year of "last day of april" comes from the
/// fixed [`NOW`]; it is `%d` in the expectation)
#[test]
fn bug69336() {
    let mut out = String::new();
    for s in [
        "last day of april",
        "last tuesday of march 2015",
        "last wednesday of march 2015",
        "last wednesday of april 2015",
        "last wednesday of march 2014",
        "last wednesday of april 2014",
    ] {
        out.push_str(&vstr(&date(
            "d.m.Y",
            strtotime_abs(s, "UTC").unwrap(),
            "UTC",
        )));
    }
    assert_textf(&out, EXP_BUG69336);
}

/// ext/date/tests/bug71889.phpt
#[test]
fn bug71889() {
    let di = php::date_interval_create("P1D").unwrap();
    assert_eq!(vstr(&interval_format(&di, "%")), "string(0) \"\"\n");
}

/// ext/date/tests/bug73294.phpt
#[test]
fn bug73294() {
    for i in -1050..=-1000 {
        let dt = DT::new(&format!("{i}-06-22 00:00:00"), "UTC");
        let expected = format!("{i}-06-22 00:00:00");
        assert_eq!(
            dt.format("Y-m-d H:i:s"),
            expected,
            "Wrong: Should have been {expected}"
        );
    }
}

/// ext/date/tests/bug73858.phpt (relative to the fixed [`NOW`]; the day counts do
/// not depend on it)
#[test]
fn bug73858() {
    let z = "UTC";
    let ss = "february first day of last month midnight";
    let es = "february first day of this month midnight - 1 second";
    let verbose = |s: &str| {
        let mut d = DT::new("now", z);
        d.set_timestamp(strtotime_abs(s, z).unwrap());
        d
    };
    let days = |iv: PhpInterval| interval_days(&iv.diff);

    let (s, e) = (DT::new(ss, z), DT::new(es, z));
    assert_eq!(days(e.diff(&s)), Some(30));
    let (s, e) = (verbose(ss), verbose(es));
    assert_eq!(days(e.diff(&s)), Some(30));
    let (s, e) = (verbose(ss), DT::new(es, z));
    assert_eq!(days(e.diff(&s)), Some(30));
    let (s, e) = (DT::new(ss, z), verbose(es));
    assert_eq!(days(e.diff(&s)), Some(30));
    let (s, e) = (DT::new(ss, z), verbose(es));
    assert_eq!(days(s.diff(&e)), Some(30));
    let s = DT::new(ss, z);
    let mut e = DT::new("february first day of this month midnight", z);
    assert!(e.modify("- 1 second"));
    assert_eq!(days(e.diff(&s)), Some(30));
}

/// ext/date/tests/bug75167.phpt
#[test]
fn bug75167() {
    let date = DT::new_in("2014-3-30 00:00:00", "Europe/London", "UTC");
    let mut five_hours_later = date.clone();
    five_hours_later.add("PT5H");
    let mut new_date = five_hours_later.clone();
    new_date.sub("PT5H");
    assert_eq!(date.format("c"), "2014-03-30T00:00:00+00:00");
    assert_eq!(new_date.format("c"), "2014-03-30T00:00:00+00:00");
}

/// ext/date/tests/bug77243-001.phpt
#[test]
fn bug77243_001() {
    let time: i64 = -62_167_046_400;
    let mut out = String::new();
    for i in -10..10 {
        out.push_str(&date("Y-m-d o-W-N", time + i * 86400, "UTC"));
        out.push('\n');
    }
    assert_text(&out, EXP_BUG77243_001);
}

/// ext/date/tests/bug78452.phpt
#[test]
fn bug78452() {
    let z = "Asia/Tehran";
    let date1 = DT::new("2019-09-24 11:47:24", z);
    let date2 = DT::new("2019-08-21 12:47:24", z);
    assert_textf(&interval_var_dump(&date1.diff(&date2), 3), EXP_BUG78452);
}

/// ext/date/tests/bug79716.phpt
#[test]
fn bug79716() {
    let datetime = DT::new_in("2770-01-00 15:00:00.000000", "UTC", "UTC");
    let mut out = V::Bool(datetime.format("j") == "0").var_dump();
    out.push_str(&datetime.var_dump("DateTimeImmutable", 1));
    assert_textf(&out, EXP_BUG79716);
}

/// ext/date/tests/bug80913.phpt
#[test]
fn bug80913() {
    let date = DT::new_in("2021-03-28 03:00:00", "Europe/Amsterdam", "UTC");
    let mut before = date.clone();
    before.sub("PT30M");
    let mut after = date.clone();
    after.add("PT30M");
    let atom = "Y-m-d\\TH:i:sP";
    let mut out = vstr(&date.format(atom));
    out.push_str(&vstr(&before.format(atom)));
    out.push_str(&vstr(&after.format(atom)));
    assert_text(&out, EXP_BUG80913);
}

/// ext/date/tests/bug81263.phpt
#[test]
fn bug81263() {
    let dt1 = DT::new_in("2020-07-19 18:30:00", "Europe/Berlin", "UTC");
    let dt2 = DT::new_in("2020-07-19 16:30:00", "UTC", "UTC");
    let pr = |iv: &PhpInterval| V::Obj("DateInterval".into(), 0, interval_props(iv)).print_r();
    let out = pr(&dt2.diff(&dt1)) + &pr(&dt1.diff(&dt2));
    assert_textf(&out, EXP_BUG81263);
}

/// ext/date/tests/call_function_from_method.phpt ("NOW" is the fixed [`NOW`]; the
/// year is `%d`)
#[test]
fn call_function_from_method() {
    let d = DT::new("NOW", "UTC");
    let out = vstr(&d.format("Y")) + &vstr(&d.format("Y"));
    assert_textf(&out, "string(4) \"%d\"\nstring(4) \"%d\"");
}

/// ext/date/tests/date-parse-by-format001.phpt
#[test]
fn date_parse_by_format001() {
    let date = "06/08/04";
    let out = date_parse_from_format("!m/d/y", date).print_r()
        + &date_parse_from_format("!m*d*y", date).print_r();
    assert_text(&out, EXP_DATE_PARSE_BY_FORMAT001);
}

/// ext/date/tests/date_constants.phpt (partial: the constant comparisons and the
/// deprecation notices are not expressible)
#[test]
fn date_constants() {
    let z = "Europe/Oslo";
    let constants = [
        "Y-m-d\\TH:i:sP",           // DATE_ATOM
        "l, d-M-Y H:i:s T",         // DATE_COOKIE
        "Y-m-d\\TH:i:sO",           // DATE_ISO8601
        "X-m-d\\TH:i:sP",           // DATE_ISO8601_EXPANDED
        "D, d M y H:i:s O",         // DATE_RFC822
        "l, d-M-y H:i:s T",         // DATE_RFC850
        "D, d M y H:i:s O",         // DATE_RFC1036
        "D, d M Y H:i:s O",         // DATE_RFC1123
        "D, d M Y H:i:s \\G\\M\\T", // DATE_RFC7231
        "D, d M Y H:i:s O",         // DATE_RFC2822
        "Y-m-d\\TH:i:sP",           // DATE_RFC3339
        "Y-m-d\\TH:i:s.vP",         // DATE_RFC3339_EXTENDED
        "D, d M Y H:i:s O",         // DATE_RSS
        "Y-m-d\\TH:i:sP",           // DATE_W3C
    ];
    let t1 = strtotime_abs("1 Jul 06 14:27:30 +0200", z).unwrap();
    let t2 = strtotime_abs("2006-05-30T14:32:13+02:00", z).unwrap();
    let mut out = String::new();
    for c in constants {
        out.push_str(&vstr(&date(c, t1, z)));
        out.push_str(&vstr(&date(c, t2, z)));
    }
    let expected: Vec<&str> = EXP_DATE_CONSTANTS.lines().skip(1).take(28).collect();
    assert_text(&out, &expected.join("\n"));
}

/// ext/date/tests/date_date_set_basic1.phpt
#[test]
fn date_date_set_basic1() {
    let mut datetime = DT::new("2009-01-30 19:34:10", "Europe/London");
    assert_eq!(
        datetime.format("D, d M Y H:i:s O"),
        "Fri, 30 Jan 2009 19:34:10 +0000"
    );
    datetime.set_date(2008, 2, 1);
    assert_eq!(
        datetime.format("D, d M Y H:i:s O"),
        "Fri, 01 Feb 2008 19:34:10 +0000"
    );
}

/// ext/date/tests/date_default_timezone_set_error.phpt (partial: the identifier
/// lookup behind the notice)
#[test]
fn date_default_timezone_set_error() {
    assert!(!timezone_id_is_valid("foo"));
}

/// ext/date/tests/date_interval_bad_format_leak.phpt
#[test]
fn date_interval_bad_format_leak() {
    let mut out = String::new();
    let e = php::date_interval_create("P3\"D").unwrap_err();
    out.push_str(&format!("DateMalformedIntervalStringException: {e}\n"));
    for s in ["P3\"D", "2008-03-01T12:00:00Z1"] {
        for func in [
            "DatePeriod::__construct",
            "DatePeriod::createFromISO8601String",
        ] {
            let e = iso_period(s, func).unwrap_err();
            out.push_str(&format!("DateMalformedPeriodStringException: {e}\n"));
        }
    }
    let expected: Vec<&str> = EXP_DATE_INTERVAL_BAD_FORMAT_LEAK
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("Deprecated:"))
        .collect();
    assert_text(&out, &expected.join("\n"));
}

/// ext/date/tests/date_offset_get_basic1.phpt
#[test]
fn date_offset_get_basic1() {
    let z = "Europe/London";
    let winter = DT::new("2008-12-25 14:25:41", z);
    let summer = DT::new("2008-07-02 14:25:41", z);
    let mut out = String::from("*** Testing date_offset_get() : basic functionality ***\n");
    out.push_str(&format!(
        "Winter offset: {} hours\n",
        winter.offset() / 3600
    ));
    out.push_str(&format!(
        "Summer offset: {} hours\n",
        summer.offset() / 3600
    ));
    assert_text(
        &out,
        "*** Testing date_offset_get() : basic functionality ***\nWinter offset: 0 hours\nSummer offset: 1 hours",
    );
}

/// ext/date/tests/date_period_bad_iso_format.phpt
#[test]
fn date_period_bad_iso_format() {
    let mut out = String::new();
    for s in ["R4", "R4/2012-07-01T00:00:00Z", "2012-07-01T00:00:00Z/P7D"] {
        for func in [
            "DatePeriod::__construct",
            "DatePeriod::createFromISO8601String",
        ] {
            let e = iso_period(s, func).unwrap_err();
            out.push_str(&format!("DateMalformedPeriodStringException: {e}\n"));
        }
    }
    let expected: Vec<&str> = EXP_DATE_PERIOD_BAD_ISO_FORMAT
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("Deprecated:"))
        .collect();
    assert_text(&out, &expected.join("\n"));
}

/// ext/date/tests/date_sub_basic.phpt
#[test]
fn date_sub_basic() {
    let mut out = String::from("*** Testing date_sub() : basic functionality ***\n");
    let format = "Y-m-d H:i:s";
    let mut d = DT::new("2008-01-01 12:25", "UTC");
    out.push_str(&vstr(&d.format(format)));
    for interval in ["P3Y6M4DT12H30M5S", "P0D", "P2DT1M", "P1Y2MT23H43M150S"] {
        d.sub(interval);
        out.push_str(&vstr(&d.format(format)));
    }
    assert_text(&out, EXP_DATE_SUB_BASIC);
}

/// ext/date/tests/date_sunrise_variation7.phpt
#[test]
fn date_sunrise_variation7() {
    let inputs: [(&str, f64, f64, f64); 9] = [
        ("Pacific/Samoa", -14.24, -170.72, -11.0),
        ("US/Alaska", 61.0, -150.0, -9.0),
        ("America/Chicago", 41.85, -87.65, -5.0),
        ("America/Montevideo", -34.88, -56.18, -3.0),
        ("Africa/Casablanca", 33.65, -7.58, 0.0),
        ("Europe/Moscow", 55.75, 37.58, 4.0),
        ("Asia/Hong_Kong", 22.28, 114.15, 8.0),
        ("Australia/Brisbane", -27.46, 153.2, 10.0),
        ("Pacific/Wallis", -13.3, -176.16, 12.0),
    ];
    let mut out = String::from("*** Testing date_sunrise() : usage variation ***\n");
    for (zone, lat, lon, gmt) in inputs {
        for day in [11, 12] {
            let time = mktime(8, 8, 8, 8, day, 2008, zone);
            let r = sunrise_sunset(
                time,
                SunFormat::String,
                lat,
                lon,
                90.0,
                Some(gmt),
                &tz(zone),
                false,
            );
            match r {
                Some(SunResult::String(s)) => out.push_str(&vstr(&s)),
                other => out.push_str(&format!("{other:?}\n")),
            }
        }
    }
    assert_text(&out, EXP_DATE_SUNRISE_VARIATION7);
}

/// ext/date/tests/date_time_fractions_serialize.phpt (partial: the serialised
/// string is not expressible; the object dump is)
#[test]
fn date_time_fractions_serialize() {
    let dt = DT::new("2016-10-03 12:47:18.819313", "UTC");
    assert_textf(
        &dt.var_dump("DateTime", 2),
        r#"object(DateTime)#2 (%d) {
  ["date"]=>
  string(26) "2016-10-03 12:47:18.819313"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#,
    );
}

/// ext/date/tests/date_timestamp_set.phpt (`date_create()` uses the fixed [`NOW`],
/// which setTimestamp() overrides)
#[test]
fn date_timestamp_set() {
    let mut dt = DT::new("now", "UTC");
    dt.set_timestamp(1_234_567_890);
    assert_eq!(
        vstr(&dt.format("B => (U) => T Y-M-d H:i:s")),
        "string(47) \"021 => (1234567890) => UTC 2009-Feb-13 23:31:30\"\n"
    );
}

/// ext/date/tests/format-negative-timestamp.phpt (deterministic pseudo-random values
/// instead of `rand()`)
#[test]
fn format_negative_timestamp() {
    let max: i64 = 2_147_483_647; // getrandmax()
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    for _ in 0..200 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let mut new_tm = 1 + ((seed >> 33) as i64 % max);
        if new_tm > max / 2 {
            new_tm = -new_tm;
        }
        assert_eq!(
            strtotime_abs(&format!("@{new_tm}"), "GMT"),
            Some(new_tm),
            "Error when parsing: @{new_tm}"
        );
    }
}

/// ext/date/tests/getdate_variation4.phpt
#[test]
fn getdate_variation4() {
    let inputs = [
        ("01 Jan 1970", 0),
        ("01 Jan 1971", 31_536_000),
        ("01 Jan 1972", 63_072_000),
        ("01 Jan 1973", 94_694_400),
    ];
    let mut out = String::from("*** Testing getdate() : usage variation ***\n");
    for (key, value) in inputs {
        out.push_str(&format!("\n--{key}--\n"));
        out.push_str(&getdate(value, "Asia/Calcutta").var_dump());
    }
    assert_text(&out, EXP_GETDATE_VARIATION4);
}

/// ext/date/tests/gh10447.phpt
#[test]
fn gh10447() {
    let date = DT::new("2023-01-25T00:00:00+00:00", "UTC");
    assert_eq!(date.format("Y-m-d\\TH:i:sp"), "2023-01-25T00:00:00Z");
    let date = DT::new("2023-01-25T00:00:00-00:00", "UTC");
    assert_eq!(date.format("Y-m-d\\TH:i:sp"), "2023-01-25T00:00:00Z");
}

/// ext/date/tests/gh14732.phpt (partial: the ValueErrors are argument validation;
/// date_sunset()/date_sunrise() with NAN return false)
#[test]
fn gh14732() {
    let utc = tz("UTC");
    // date.sunset_zenith / date.sunrise_zenith default
    let zenith = 90.833_333;
    assert_eq!(
        sunrise_sunset(
            1,
            SunFormat::String,
            f64::NAN,
            1.0,
            zenith,
            None,
            &utc,
            true
        ),
        None
    );
    assert_eq!(
        sunrise_sunset(
            1,
            SunFormat::String,
            1.0,
            f64::NAN,
            zenith,
            None,
            &utc,
            false
        ),
        None
    );
}

/// ext/date/tests/gh9880.phpt (`new DateTime()` uses the fixed [`NOW`], which
/// setTimestamp() overrides)
#[test]
fn gh9880() {
    let z = "America/Los_Angeles";
    let mut now_time = DT::new("now", z);
    now_time.set_timestamp(1_667_416_695);
    let mut date_time = DT::new("now", z);
    date_time.set_timestamp(1_671_904_800);
    date_time.set_timezone("America/New_York");
    assert_eq!(
        interval_format(&date_time.diff(&now_time), "%R %a %H %I %S"),
        "- 51 22 41 45"
    );
}

/// ext/date/tests/gmdate_variation12_64bits.phpt
#[test]
fn gmdate_variation12_64bits() {
    let iso = "Y-m-d\\TH:i:sO";
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    out.push_str("\n-- Testing gmdate() function with minimum range of timestamp --\n");
    out.push_str(&vstr(&gmdate(iso, mktime(20, 45, 54, 12, 13, 1901, "UTC"))));
    out.push_str("\n-- Testing gmdate() function with less than the range of timestamp --\n");
    out.push_str(&vstr(&gmdate(iso, mktime(20, 45, 50, 12, 13, 1901, "UTC"))));
    out.push_str("\n-- Testing gmdate() function with maximum range of timestamp --\n");
    out.push_str(&vstr(&gmdate(iso, mktime(3, 14, 7, 1, 19, 2038, "UTC"))));
    out.push_str("\n-- Testing gmdate() function with greater than the range of timestamp --\n");
    out.push_str(&vstr(&gmdate(iso, mktime(3, 14, 10, 1, 19, 2038, "UTC"))));
    assert_text(&out, EXP_GMDATE_VARIATION12_64BITS);
}

/// ext/date/tests/gmdate_variation7.phpt (calls without a timestamp use the fixed
/// [`NOW`]; they are `%d` in the expectation)
#[test]
fn gmdate_variation7() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let non_leap = mktime(8, 8, 8, 8, 8, 2007, "UTC");
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    out.push_str(
        "\n-- Testing gmdate() function with checking non leap year using Leap Year format --\n",
    );
    out.push_str(&vstr(&gmdate("L", non_leap)));
    out.push_str(
        "\n-- Testing gmdate() function with checking leap year using Leap Year format --\n",
    );
    out.push_str(&vstr(&gmdate("L", NOW)));
    out.push_str(&vstr(&gmdate("L", timestamp)));
    out.push_str("\n-- Testing gmdate() function with ISO-8601 year number format --\n");
    out.push_str(&vstr(&gmdate("o", NOW)));
    out.push_str(&vstr(&gmdate("o", timestamp)));
    out.push_str(
        "\n-- Testing gmdate() function with full numeric representation of year format --\n",
    );
    out.push_str(&vstr(&gmdate("Y", NOW)));
    out.push_str(&vstr(&gmdate("Y", timestamp)));
    out.push_str("\n-- Testing gmdate() function with 2 digit representation year format --\n");
    out.push_str(&vstr(&gmdate("y", NOW)));
    out.push_str(&vstr(&gmdate("y", timestamp)));
    assert_textf(&out, EXP_GMDATE_VARIATION7);
}

/// ext/date/tests/idate_basic.phpt (`idate('Y')` at the fixed [`NOW`]; `int(%d)`)
#[test]
fn idate_basic() {
    let y = idate('Y', NOW, "Asia/Calcutta").unwrap();
    let out = format!(
        "*** Testing idate() : basic functionality ***\n{}",
        V::Int(i64::from(y)).var_dump()
    );
    assert_textf(
        &out,
        "*** Testing idate() : basic functionality ***\nint(%d)",
    );
}

/// ext/date/tests/mktime-3-64bit.phpt
#[test]
fn mktime_3_64bit() {
    let years = [
        0, 69, 70, 71, 99, 100, 101, 105, 110, 1900, 1901, 1902, 1999, 2000, 2001,
    ];
    let mut out = String::new();
    for zone in ["America/Toronto", "Europe/Oslo"] {
        out.push_str(zone);
        out.push('\n');
        for year in years {
            out.push_str(&format!("Y: {year:4} - "));
            let ret = php::php_mktime(
                1,
                Some(1),
                Some(1),
                Some(1),
                Some(1),
                Some(year),
                NOW,
                Some(&tz(zone)),
            );
            match ret {
                None | Some(0) => out.push_str("out of range\n"),
                Some(ret) => {
                    out.push_str(&date("F Y-m-d\\TH:i:sO", ret, zone));
                    out.push('\n');
                }
            }
        }
        out.push('\n');
    }
    assert_textf(&out, EXP_MKTIME_3_64BIT);
}

/// ext/date/tests/oo_002.phpt (the subclasses add nothing; clone is a copy)
#[test]
fn oo_002() {
    let rfc822 = "D, d M y H:i:s O";
    let z = "Europe/Berlin";
    let mut d = DT::new("1pm Aug 1 GMT 2007", z);
    let mut out = vstr(&d.format(rfc822));
    let mut c = d.clone();
    out.push_str(&vstr(&c.format(rfc822)));
    assert!(d.modify("1 hour"));
    assert!(c.modify("1 second ago"));
    out.push_str(&vstr(&d.format(rfc822)));
    out.push_str(&vstr(&c.format(rfc822)));
    let t = php::timezone_open("Asia/Tokyo").unwrap();
    out.push_str(&vstr(&t.name()));
    let c = t.clone();
    out.push_str(&vstr(&c.name()));
    assert_text(&out, EXP_OO_002);
}

/// ext/date/tests/oom_dateinterval_construct.phpt
#[test]
fn oom_dateinterval_construct() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    let e = php::date_interval_create(&s).unwrap_err();
    assert_textf(
        &format!("DateMalformedIntervalStringException: {e}"),
        "DateMalformedIntervalStringException: Unknown or bad format (%s)",
    );
}

/// ext/date/tests/oom_datetimeimmutable_modify.phpt (`new DateTimeImmutable()` uses
/// the fixed [`NOW`]; the modification fails before it matters)
#[test]
fn oom_datetimeimmutable_modify() {
    let mut d = DT::new("now", "UTC");
    let s = vec![1u8; 4 * 1024 * 1024];
    let errs = php::date_modify(&mut d.t, &s).unwrap_err();
    let e = &errs.errors[0];
    let msg = format!(
        "DateMalformedStringException: DateTimeImmutable::modify(): Failed to parse time string (...) at position {} ({}): {}",
        e.position, e.character as char, e.message
    );
    assert_textf(
        &msg,
        "DateMalformedStringException: DateTimeImmutable::modify(): Failed to parse time string (%s) at position 0 (%s): Unexpected character",
    );
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type3-bd2.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type3_bd2() {
    let date_format = "Y-m-d H:i:s T e";
    let interval_fmt = "P%dDT%hH";
    let cases = [
        (
            "bd0",
            "2010-11-07 01:00:00 EST",
            "2010-11-07 01:59:59",
            "P%dDT%hH%iM%sS",
        ),
        (
            "bd5",
            "2010-11-07 01:30:00 EST",
            "2010-11-06 04:30:00",
            interval_fmt,
        ),
        (
            "bd6",
            "2010-11-07 01:30:00 EDT",
            "2010-11-06 04:30:00",
            interval_fmt,
        ),
        (
            "bd8a",
            "2010-11-07 01:00:00 EST",
            "2010-11-06 01:00:00",
            interval_fmt,
        ),
        (
            "bd8b",
            "2010-11-07 01:00:00 EDT",
            "2010-11-06 01:00:00",
            interval_fmt,
        ),
    ];
    let mut out = String::new();
    for (name, end_s, start_s, f) in cases {
        let mut end = DT::new(end_s, NY);
        end.set_timezone(NY);
        let start = DT::new(start_s, NY);
        out.push_str(&format!(
            "{name} {} - {} = {}\n",
            end.format(date_format),
            start.format(date_format),
            interval_format(&start.diff(&end), f)
        ));
    }
    assert_text(&out, EXP_RFC_DATETIME_AND_DAYLIGHT_SAVING_TIME_TYPE3_BD2);
}

/// ext/date/tests/strtotime-mysql-64bit.phpt
#[test]
fn strtotime_mysql_64bit() {
    let mut out = String::new();
    for d in ["19970523091528", "20001231185859", "20800410101010"] {
        match strtotime_abs(d, "UTC") {
            Some(t) => out.push_str(&vstr(&date("r", t, "UTC"))),
            None => out.push_str("bool(false)\n"),
        }
    }
    assert_text(&out, EXP_STRTOTIME_MYSQL_64BIT);
}

/// ext/date/tests/strtotime3.phpt (partial: a 32-bit only test; the "t0222 t0222"
/// line, which overflows a 32-bit integer, is left out)
#[test]
fn strtotime3() {
    let z = "Europe/Lisbon";
    let time = 1_150_494_719;
    let strs = [
        "",
        " \t\r\n000",
        "yesterday",
        "22:49:12",
        "22:49:12 bogusTZ",
        "22.49.12.42GMT",
        "22.49.12.42bogusTZ",
        "t0222",
        // "t0222 t0222": 32-bit overflow
        "022233",
        "022233 bogusTZ",
        "2-3-2004",
        "2.3.2004",
        "20060212T23:12:23UTC",
        "20060212T23:12:23 bogusTZ",
        "2006167",
        "Jan-15-2006",
        "2006-Jan-15",
        "10/Oct/2000:13:55:36 +0100",
        "10/Oct/2000:13:55:36 +00100",
        "2006",
        "1986",
        "JAN",
        "January",
    ];
    let mut out = String::new();
    for s in strs {
        match strtotime(s, time, z) {
            Some(t) => out.push_str(&vstr(&date("D, d M Y H:i:s O", t, z))),
            None => out.push_str("bool(false)\n"),
        }
    }
    let mut expected: Vec<&str> = EXP_STRTOTIME3
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with("Warning:"))
        .collect();
    expected.remove(8);
    assert_text(&out, &expected.join("\n"));
}

/// ext/date/tests/test-parse-from-format.phpt
#[test]
fn test_parse_from_format() {
    let z = "Europe/Oslo";
    let formats = [
        "Y-m-d\\TH:i:sP",   // DATE_ATOM
        "l, d-M-Y H:i:s T", // DATE_COOKIE
        "Y-m-d\\TH:i:sO",   // DATE_ISO8601
        "X-m-d\\TH:i:sP",   // DATE_ISO8601_EXPANDED
        "D, d M y H:i:s O", // DATE_RFC822
        "l, d-M-y H:i:s T", // DATE_RFC850
        "D, d M y H:i:s O", // DATE_RFC1036
        "D, d M Y H:i:s O", // DATE_RFC1123
        "D, d M Y H:i:s O", // DATE_RFC2822
        "Y-m-d\\TH:i:sP",   // DATE_RFC3339
        "Y-m-d\\TH:i:s.vP", // DATE_RFC3339_EXTENDED
        "D, d M Y H:i:s O", // DATE_RSS
        "Y-m-d\\TH:i:sP",   // DATE_W3C
    ];
    // Object handles as PHP reuses them.
    let ids = [2, 1, 3, 2, 1, 3, 2, 1, 3, 2, 1, 3, 2];
    let mut out = String::new();
    for (format, id) in formats.iter().zip(ids) {
        let date = DT::new("2008-07-08T22:14:12+02:00", z);
        let formatted = date.format(format);
        out.push_str(&vstr(format));
        out.push_str(&vstr(&formatted));
        match DT::from_format(format, &formatted, None, z) {
            Some(date2) => out.push_str(&date2.var_dump("DateTime", id)),
            None => {
                out.push_str("bool(false)\n\n");
                out.push_str(&date_parse_from_format(format, &formatted).var_dump());
                continue;
            }
        }
        out.push('\n');
    }
    assert_text(&out, EXP_TEST_PARSE_FROM_FORMAT);
}

/// ext/date/tests/timezone_open_warning.phpt
#[test]
fn timezone_open_warning() {
    let mut out = String::new();
    for timezone in ["+02:30", "Europe/Kyiv", "2.5", "99:60", "Europe/Lviv"] {
        match php::timezone_open(timezone) {
            Ok(d) => out.push_str(&format!("In: {timezone}; Out: {}\n", d.name())),
            Err(w) => out.push_str(&format!(
                "\nWarning: timezone_open(): {w} in %stimezone_open_warning.php on line 6\n"
            )),
        }
    }
    assert_eq!(out.trim_end(), EXP_TIMEZONE_OPEN_WARNING.trim_end());
}

// ---------------------------------------------------------------------------------
// Expected outputs (verbatim from the phpt files)

/// Expected output of `012.phpt`.
const EXP_012: &str = r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2006-01-23 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
string(19) "2006/01/23 00:00:00"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2006-01-30 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
string(19) "2006/01/30 00:00:00"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2007-12-10 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
string(19) "2007/12/10 00:00:00"
Done"#;

/// Expected output of `DateTimeImmutable_createFromInterface-001.phpt`.
const EXP_DATETIMEIMMUTABLE_CREATEFROMINTERFACE_001: &str = r#"object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}
object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}"#;

/// Expected output of `DateTime_add-fall-type2-type3.phpt`.
const EXP_DATETIME_ADD_FALL_TYPE2_TYPE3: &str = r#"test_time_fall_type2_prev_type3_prev: ADD: 2010-10-04 02:18:48 EDT + P+0Y1M2DT16H19M40S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type3_dt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_prev_type3_redodt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_prev_type3_redost: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT7H36M16S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_prev_type3_st: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT9H38M27S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_prev_type3_post: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EDT**
test_time_fall_type2_dt_type3_prev: ADD: 2010-11-07 00:10:20 EDT + P-0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_dt_type3_dt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT0H5M15S = **2010-11-07 00:15:35 EDT**
test_time_fall_type2_dt_type3_redodt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_dt_type3_redost: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT2H4M24S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_dt_type3_st: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT4H6M35S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_dt_type3_post: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EDT**
test_time_fall_type2_redodt_type3_prev: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_redodt_type3_dt: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_redodt_type3_redodt: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT0H3M2S = **2010-11-07 01:15:35 EDT**
test_time_fall_type2_redodt_type3_redost: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT1H2M11S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_redodt_type3_st: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT3H4M22S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_redodt_type3_post: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EDT**
test_time_fall_type2_redost_type3_prev: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT7H36M16S = **2010-11-06 17:38:28 EST**
test_time_fall_type2_redost_type3_dt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT2H4M24S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_redost_type3_redodt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT1H2M11S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_redost_type3_redost: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT0H2M10S = **2010-11-07 01:16:54 EST**
test_time_fall_type2_redost_type3_st: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_redost_type3_post: ADD: 2010-11-07 01:14:44 EST + P+0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_st_type3_prev: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT9H38M27S = **2010-11-06 17:38:28 EST**
test_time_fall_type2_st_type3_dt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT4H6M35S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_st_type3_redodt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT3H4M22S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_st_type3_redost: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_st_type3_st: ADD: 2010-11-07 03:16:55 EST + P+0Y0M0DT2H3M1S = **2010-11-07 05:19:56 EST**
test_time_fall_type2_st_type3_post: ADD: 2010-11-07 03:16:55 EST + P+0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_post_type3_prev: ADD: 2010-11-08 19:59:59 EST + P-0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EST**
test_time_fall_type2_post_type3_dt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT20H49M39S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_post_type3_redodt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT19H47M26S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_post_type3_redost: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_post_type3_st: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_post_type3_post: ADD: 2010-11-08 18:57:55 EST + P+0Y0M0DT1H2M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_dtsec_type3_stsec: ADD: 2010-11-07 01:59:59 EDT + P+0Y0M0DT0H0M1S = **2010-11-07 02:00:00 EDT**
test_time_fall_type2_stsec_type3_dtsec: ADD: 2010-11-07 01:00:00 EST + P-0Y0M0DT0H0M1S = **2010-11-07 00:59:59 EST**"#;

/// Expected output of `DateTime_add-spring-type2-type3.phpt`.
const EXP_DATETIME_ADD_SPRING_TYPE2_TYPE3: &str = r#"test_time_spring_type2_prev_type3_prev: ADD: 2010-02-11 02:18:48 EST + P+0Y1M2DT16H19M40S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_prev_type3_st: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_prev_type3_dt: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT7H38M27S = **2010-03-14 02:16:55 EST**
test_time_spring_type2_prev_type3_post: ADD: 2010-03-13 18:38:28 EST + P+0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EST**
test_time_spring_type2_st_type3_prev: ADD: 2010-03-14 00:10:20 EST + P-0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_st_type3_st: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT0H5M15S = **2010-03-14 00:15:35 EST**
test_time_spring_type2_st_type3_dt: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT2H6M35S = **2010-03-14 02:16:55 EST**
test_time_spring_type2_st_type3_post: ADD: 2010-03-14 00:10:20 EST + P+0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EST**
test_time_spring_type2_dt_type3_prev: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT7H38M27S = **2010-03-13 19:38:28 EDT**
test_time_spring_type2_dt_type3_st: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT2H6M35S = **2010-03-14 01:10:20 EDT**
test_time_spring_type2_dt_type3_dt: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M0DT2H3M1S = **2010-03-14 05:19:56 EDT**
test_time_spring_type2_dt_type3_post: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type2_post_type3_prev: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EDT**
test_time_spring_type2_post_type3_st: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT18H49M39S = **2010-03-14 01:10:20 EDT**
test_time_spring_type2_post_type3_dt: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_post_type3_post: ADD: 2010-03-15 18:57:55 EDT + P+0Y0M0DT1H2M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type2_stsec_type3_dtsec: ADD: 2010-03-14 01:59:59 EST + P+0Y0M0DT0H0M1S = **2010-03-14 02:00:00 EST**
test_time_spring_type2_dtsec_type3_stsec: ADD: 2010-03-14 03:00:00 EDT + P-0Y0M0DT0H0M1S = **2010-03-14 02:59:59 EDT**"#;

/// Expected output of `DateTime_days-dates.phpt`.
const EXP_DATETIME_DAYS_DATES: &str = r#"test__7: DAYS: **7**
test_years_positive__7_by_0_day: DAYS: **2557**
test_years_positive__7_by_1_day: DAYS: **2558**
test_years_positive__6_shy_1_day: DAYS: **2556**
test_years_positive__7_by_1_month: DAYS: **2585**
test_years_positive__6_shy_1_month: DAYS: **2526**
test_years_positive__7_by_1_month_split_newyear: DAYS: **2588**
test_years_positive__6_shy_1_month_split_newyear: DAYS: **2526**
test_negative__7: DAYS: **7**
test_years_negative__7_by_0_day: DAYS: **2557**
test_years_negative__7_by_1_day: DAYS: **2558**
test_years_negative__6_shy_1_day: DAYS: **2556**
test_years_negative__7_by_1_month: DAYS: **2585**
test_years_negative__6_shy_1_month: DAYS: **2526**
test_years_negative__7_by_1_month_split_newyear: DAYS: **2588**
test_years_negative__6_shy_1_month_split_newyear: DAYS: **2526**"#;

/// Expected output of `DateTime_diff-dates.phpt`.
const EXP_DATETIME_DIFF_DATES: &str = r#"test__7: DIFF: 2009-01-14 00:00:00 EST - 2009-01-07 00:00:00 EST = **P+0Y0M7DT0H0M0S**
test_years_positive__7_by_0_day: DIFF: 2007-02-07 00:00:00 EST - 2000-02-07 00:00:00 EST = **P+7Y0M0DT0H0M0S**
test_years_positive__7_by_1_day: DIFF: 2007-02-08 00:00:00 EST - 2000-02-07 00:00:00 EST = **P+7Y0M1DT0H0M0S**
test_years_positive__6_shy_1_day: DIFF: 2007-02-06 00:00:00 EST - 2000-02-07 00:00:00 EST = **P+6Y11M30DT0H0M0S**
test_years_positive__7_by_1_month: DIFF: 2007-03-07 00:00:00 EST - 2000-02-07 00:00:00 EST = **P+7Y1M0DT0H0M0S**
test_years_positive__6_shy_1_month: DIFF: 2007-01-07 00:00:00 EST - 2000-02-07 00:00:00 EST = **P+6Y11M0DT0H0M0S**
test_years_positive__7_by_1_month_split_newyear: DIFF: 2007-01-07 00:00:00 EST - 1999-12-07 00:00:00 EST = **P+7Y1M0DT0H0M0S**
test_years_positive__6_shy_1_month_split_newyear: DIFF: 2006-12-07 00:00:00 EST - 2000-01-07 00:00:00 EST = **P+6Y11M0DT0H0M0S**
test_negative__7: DIFF: 2009-01-07 00:00:00 EST - 2009-01-14 00:00:00 EST = **P-0Y0M7DT0H0M0S**
test_years_negative__7_by_0_day: DIFF: 2000-02-07 00:00:00 EST - 2007-02-07 00:00:00 EST = **P-7Y0M0DT0H0M0S**
test_years_negative__7_by_1_day: DIFF: 2000-02-07 00:00:00 EST - 2007-02-08 00:00:00 EST = **P-7Y0M1DT0H0M0S**
test_years_negative__6_shy_1_day: DIFF: 2000-02-07 00:00:00 EST - 2007-02-06 00:00:00 EST = **P-6Y11M28DT0H0M0S**
test_years_negative__7_by_1_month: DIFF: 2000-02-07 00:00:00 EST - 2007-03-07 00:00:00 EST = **P-7Y1M0DT0H0M0S**
test_years_negative__6_shy_1_month: DIFF: 2000-02-07 00:00:00 EST - 2007-01-07 00:00:00 EST = **P-6Y11M0DT0H0M0S**
test_years_negative__7_by_1_month_split_newyear: DIFF: 1999-12-07 00:00:00 EST - 2007-01-07 00:00:00 EST = **P-7Y1M0DT0H0M0S**
test_years_negative__6_shy_1_month_split_newyear: DIFF: 2000-01-07 00:00:00 EST - 2006-12-07 00:00:00 EST = **P-6Y11M0DT0H0M0S**"#;

/// Expected output of `DateTime_sub-dates.phpt`.
const EXP_DATETIME_SUB_DATES: &str = r#"test__7: SUB: 2009-01-14 00:00:00 EST - P+0Y0M7DT0H0M0S = **2009-01-07 00:00:00 EST**
test_years_positive__7_by_0_day: SUB: 2007-02-07 00:00:00 EST - P+7Y0M0DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_positive__7_by_1_day: SUB: 2007-02-08 00:00:00 EST - P+7Y0M1DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_positive__6_shy_1_day: SUB: 2007-02-06 00:00:00 EST - P+6Y11M30DT0H0M0S = **2000-02-05 00:00:00 EST**
test_years_positive__7_by_1_month: SUB: 2007-03-07 00:00:00 EST - P+7Y1M0DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_positive__6_shy_1_month: SUB: 2007-01-07 00:00:00 EST - P+6Y11M0DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_positive__7_by_1_month_split_newyear: SUB: 2007-01-07 00:00:00 EST - P+7Y1M0DT0H0M0S = **1999-12-07 00:00:00 EST**
test_years_positive__6_shy_1_month_split_newyear: SUB: 2006-12-07 00:00:00 EST - P+6Y11M0DT0H0M0S = **2000-01-07 00:00:00 EST**
test_negative__7: SUB: 2009-01-07 00:00:00 EST - P-0Y0M7DT0H0M0S = **2009-01-14 00:00:00 EST**
test_years_negative__7_by_0_day: SUB: 2000-02-07 00:00:00 EST - P-7Y0M0DT0H0M0S = **2007-02-07 00:00:00 EST**
test_years_negative__7_by_1_day: SUB: 2000-02-07 00:00:00 EST - P-7Y0M1DT0H0M0S = **2007-02-08 00:00:00 EST**
test_years_negative__6_shy_1_day: SUB: 2000-02-07 00:00:00 EST - P-6Y11M28DT0H0M0S = **2007-02-04 00:00:00 EST**
test_years_negative__7_by_1_month: SUB: 2000-02-07 00:00:00 EST - P-7Y1M0DT0H0M0S = **2007-03-07 00:00:00 EST**
test_years_negative__6_shy_1_month: SUB: 2000-02-07 00:00:00 EST - P-6Y11M0DT0H0M0S = **2007-01-07 00:00:00 EST**
test_years_negative__7_by_1_month_split_newyear: SUB: 1999-12-07 00:00:00 EST - P-7Y1M0DT0H0M0S = **2007-01-07 00:00:00 EST**
test_years_negative__6_shy_1_month_split_newyear: SUB: 2000-01-07 00:00:00 EST - P-6Y11M0DT0H0M0S = **2006-12-07 00:00:00 EST**"#;

/// Expected output of `DateTime_days-massive.phpt`.
const EXP_DATETIME_DAYS_MASSIVE: &str = r#"test_massive_positive: DAYS: **243494757**
test_massive_negative: DAYS: **243494757**"#;

/// Expected output of `DateTime_diff-massive.phpt`.
const EXP_DATETIME_DIFF_MASSIVE: &str = r#"test_massive_positive: DIFF: 333333-01-01 16:18:02 %s - -333333-01-01 16:18:02 %s = **P+666666Y0M0DT0H%s**
test_massive_negative: DIFF: -333333-01-01 16:18:02 %s - 333333-01-01 16:18:02 %s = **P-666666Y0M0DT0H%s**"#;

/// Expected output of `DateTime_sub-massive.phpt`.
const EXP_DATETIME_SUB_MASSIVE: &str = r#"test_massive_positive: SUB: 333333-01-01 16:18:02 %s - P+666666Y0M0DT0H0M0S = **-333333-01-01 16:18:02 %s**
test_massive_negative: SUB: -333333-01-01 16:18:02 %s - P-666666Y0M0DT0H0M0S = **333333-01-01 16:18:02 %s**"#;

/// Expected output of `bug30096.phpt`.
const EXP_BUG30096: &str = r#"no dst --> dst
gmmktime(1,0,0,3,27,2005): 1111885200 | gmdate('r', 1111885200):Sun, 27 Mar 2005 01:00:00 +0000
gmmktime(2,0,0,3,27,2005): 1111888800 | gmdate('r', 1111888800):Sun, 27 Mar 2005 02:00:00 +0000 | Diff: 3600
gmmktime(3,0,0,3,27,2005): 1111892400 | gmdate('r', 1111892400):Sun, 27 Mar 2005 03:00:00 +0000 | Diff: 3600
gmmktime(4,0,0,3,27,2005): 1111896000 | gmdate('r', 1111896000):Sun, 27 Mar 2005 04:00:00 +0000 | Diff: 3600

dst --> no dst
gmmktime(1,0,0,10,30,2005): 1130634000 | gmdate('r', 1130634000):Sun, 30 Oct 2005 01:00:00 +0000
gmmktime(2,0,0,10,30,2005): 1130637600 | gmdate('r', 1130637600):Sun, 30 Oct 2005 02:00:00 +0000 | Diff: 3600
gmmktime(3,0,0,10,30,2005): 1130641200 | gmdate('r', 1130641200):Sun, 30 Oct 2005 03:00:00 +0000 | Diff: 3600
gmmktime(4,0,0,10,30,2005): 1130644800 | gmdate('r', 1130644800):Sun, 30 Oct 2005 04:00:00 +0000 | Diff: 3600"#;

/// Expected output of `bug33957.phpt`.
const EXP_BUG33957: &str = r#"1992-12-14  51
1992-12-15  51
1992-12-16  51
1992-12-17  51
1992-12-18  51
1992-12-19  51
1992-12-20  51
1992-12-21  52
1992-12-22  52
1992-12-23  52
1992-12-24  52
1992-12-25  52
1992-12-26  52
1992-12-27  52
1992-12-28  53
1992-12-29  53
1992-12-30  53
1992-12-31  53
1993-01-1  53
1993-01-2  53
1993-01-3  53
1993-01-4  01
1993-01-5  01
1993-01-6  01
1993-01-7  01
1993-01-8  01
----
             1992     1993     1994     1995     1996     1997     1998     1999     2000     2001     2002     2003     2004     2005     2006     2007     2008     2009     2010     2011     2012     2013     2014     2015     2016     2017     2018     2019     
   (12-14)  51-1992  50-1993  50-1994  50-1995  50-1996  50-1997  51-1998  50-1999  50-2000  50-2001  50-2002  50-2003  51-2004  50-2005  50-2006  50-2007  50-2008  51-2009  50-2010  50-2011  50-2012  50-2013  50-2014  51-2015  50-2016  50-2017  50-2018  50-2019 
   (12-15)  51-1992  50-1993  50-1994  50-1995  50-1996  51-1997  51-1998  50-1999  50-2000  50-2001  50-2002  51-2003  51-2004  50-2005  50-2006  50-2007  51-2008  51-2009  50-2010  50-2011  50-2012  50-2013  51-2014  51-2015  50-2016  50-2017  50-2018  50-2019 
   (12-16)  51-1992  50-1993  50-1994  50-1995  51-1996  51-1997  51-1998  50-1999  50-2000  50-2001  51-2002  51-2003  51-2004  50-2005  50-2006  50-2007  51-2008  51-2009  50-2010  50-2011  50-2012  51-2013  51-2014  51-2015  50-2016  50-2017  50-2018  51-2019 
   (12-17)  51-1992  50-1993  50-1994  50-1995  51-1996  51-1997  51-1998  50-1999  50-2000  51-2001  51-2002  51-2003  51-2004  50-2005  50-2006  51-2007  51-2008  51-2009  50-2010  50-2011  51-2012  51-2013  51-2014  51-2015  50-2016  50-2017  51-2018  51-2019 
   (12-18)  51-1992  50-1993  50-1994  51-1995  51-1996  51-1997  51-1998  50-1999  51-2000  51-2001  51-2002  51-2003  51-2004  50-2005  51-2006  51-2007  51-2008  51-2009  50-2010  50-2011  51-2012  51-2013  51-2014  51-2015  50-2016  51-2017  51-2018  51-2019 
   (12-19)  51-1992  50-1993  51-1994  51-1995  51-1996  51-1997  51-1998  50-1999  51-2000  51-2001  51-2002  51-2003  51-2004  51-2005  51-2006  51-2007  51-2008  51-2009  50-2010  51-2011  51-2012  51-2013  51-2014  51-2015  51-2016  51-2017  51-2018  51-2019 
   (12-20)  51-1992  51-1993  51-1994  51-1995  51-1996  51-1997  51-1998  51-1999  51-2000  51-2001  51-2002  51-2003  52-2004  51-2005  51-2006  51-2007  51-2008  51-2009  51-2010  51-2011  51-2012  51-2013  51-2014  51-2015  51-2016  51-2017  51-2018  51-2019 
   (12-21)  52-1992  51-1993  51-1994  51-1995  51-1996  51-1997  52-1998  51-1999  51-2000  51-2001  51-2002  51-2003  52-2004  51-2005  51-2006  51-2007  51-2008  52-2009  51-2010  51-2011  51-2012  51-2013  51-2014  52-2015  51-2016  51-2017  51-2018  51-2019 
   (12-22)  52-1992  51-1993  51-1994  51-1995  51-1996  52-1997  52-1998  51-1999  51-2000  51-2001  51-2002  52-2003  52-2004  51-2005  51-2006  51-2007  52-2008  52-2009  51-2010  51-2011  51-2012  51-2013  52-2014  52-2015  51-2016  51-2017  51-2018  51-2019 
   (12-23)  52-1992  51-1993  51-1994  51-1995  52-1996  52-1997  52-1998  51-1999  51-2000  51-2001  52-2002  52-2003  52-2004  51-2005  51-2006  51-2007  52-2008  52-2009  51-2010  51-2011  51-2012  52-2013  52-2014  52-2015  51-2016  51-2017  51-2018  52-2019 
   (12-24)  52-1992  51-1993  51-1994  51-1995  52-1996  52-1997  52-1998  51-1999  51-2000  52-2001  52-2002  52-2003  52-2004  51-2005  51-2006  52-2007  52-2008  52-2009  51-2010  51-2011  52-2012  52-2013  52-2014  52-2015  51-2016  51-2017  52-2018  52-2019 
   (12-25)  52-1992  51-1993  51-1994  52-1995  52-1996  52-1997  52-1998  51-1999  52-2000  52-2001  52-2002  52-2003  52-2004  51-2005  52-2006  52-2007  52-2008  52-2009  51-2010  51-2011  52-2012  52-2013  52-2014  52-2015  51-2016  52-2017  52-2018  52-2019 
   (12-26)  52-1992  51-1993  52-1994  52-1995  52-1996  52-1997  52-1998  51-1999  52-2000  52-2001  52-2002  52-2003  52-2004  52-2005  52-2006  52-2007  52-2008  52-2009  51-2010  52-2011  52-2012  52-2013  52-2014  52-2015  52-2016  52-2017  52-2018  52-2019 
   (12-27)  52-1992  52-1993  52-1994  52-1995  52-1996  52-1997  52-1998  52-1999  52-2000  52-2001  52-2002  52-2003  53-2004  52-2005  52-2006  52-2007  52-2008  52-2009  52-2010  52-2011  52-2012  52-2013  52-2014  52-2015  52-2016  52-2017  52-2018  52-2019 
   (12-28)  53-1992  52-1993  52-1994  52-1995  52-1996  52-1997  53-1998  52-1999  52-2000  52-2001  52-2002  52-2003  53-2004  52-2005  52-2006  52-2007  52-2008  53-2009  52-2010  52-2011  52-2012  52-2013  52-2014  53-2015  52-2016  52-2017  52-2018  52-2019 
   (12-29)  53-1992  52-1993  52-1994  52-1995  52-1996  01-1998  53-1998  52-1999  52-2000  52-2001  52-2002  01-2004  53-2004  52-2005  52-2006  52-2007  01-2009  53-2009  52-2010  52-2011  52-2012  52-2013  01-2015  53-2015  52-2016  52-2017  52-2018  52-2019 
   (12-30)  53-1992  52-1993  52-1994  52-1995  01-1997  01-1998  53-1998  52-1999  52-2000  52-2001  01-2003  01-2004  53-2004  52-2005  52-2006  52-2007  01-2009  53-2009  52-2010  52-2011  52-2012  01-2014  01-2015  53-2015  52-2016  52-2017  52-2018  01-2020 
   (12-31)  53-1992  52-1993  52-1994  52-1995  01-1997  01-1998  53-1998  52-1999  52-2000  01-2002  01-2003  01-2004  53-2004  52-2005  52-2006  01-2008  01-2009  53-2009  52-2010  52-2011  01-2013  01-2014  01-2015  53-2015  52-2016  52-2017  01-2019  01-2020 
+1 (01-01)  53-1992  52-1993  52-1994  01-1996  01-1997  01-1998  53-1998  52-1999  01-2001  01-2002  01-2003  01-2004  53-2004  52-2005  01-2007  01-2008  01-2009  53-2009  52-2010  52-2011  01-2013  01-2014  01-2015  53-2015  52-2016  01-2018  01-2019  01-2020 
+1 (01-02)  53-1992  52-1993  01-1995  01-1996  01-1997  01-1998  53-1998  52-1999  01-2001  01-2002  01-2003  01-2004  53-2004  01-2006  01-2007  01-2008  01-2009  53-2009  52-2010  01-2012  01-2013  01-2014  01-2015  53-2015  01-2017  01-2018  01-2019  01-2020 
+1 (01-03)  53-1992  01-1994  01-1995  01-1996  01-1997  01-1998  53-1998  01-2000  01-2001  01-2002  01-2003  01-2004  01-2005  01-2006  01-2007  01-2008  01-2009  53-2009  01-2011  01-2012  01-2013  01-2014  01-2015  53-2015  01-2017  01-2018  01-2019  01-2020 
+1 (01-04)  01-1993  01-1994  01-1995  01-1996  01-1997  01-1998  01-1999  01-2000  01-2001  01-2002  01-2003  01-2004  01-2005  01-2006  01-2007  01-2008  01-2009  01-2010  01-2011  01-2012  01-2013  01-2014  01-2015  01-2016  01-2017  01-2018  01-2019  01-2020 
+1 (01-05)  01-1993  01-1994  01-1995  01-1996  01-1997  02-1998  01-1999  01-2000  01-2001  01-2002  01-2003  02-2004  01-2005  01-2006  01-2007  01-2008  02-2009  01-2010  01-2011  01-2012  01-2013  01-2014  02-2015  01-2016  01-2017  01-2018  01-2019  01-2020 
+1 (01-06)  01-1993  01-1994  01-1995  01-1996  02-1997  02-1998  01-1999  01-2000  01-2001  01-2002  02-2003  02-2004  01-2005  01-2006  01-2007  01-2008  02-2009  01-2010  01-2011  01-2012  01-2013  02-2014  02-2015  01-2016  01-2017  01-2018  01-2019  02-2020 
+1 (01-07)  01-1993  01-1994  01-1995  01-1996  02-1997  02-1998  01-1999  01-2000  01-2001  02-2002  02-2003  02-2004  01-2005  01-2006  01-2007  02-2008  02-2009  01-2010  01-2011  01-2012  02-2013  02-2014  02-2015  01-2016  01-2017  01-2018  02-2019  02-2020 
+1 (01-08)  01-1993  01-1994  01-1995  02-1996  02-1997  02-1998  01-1999  01-2000  02-2001  02-2002  02-2003  02-2004  01-2005  01-2006  02-2007  02-2008  02-2009  01-2010  01-2011  01-2012  02-2013  02-2014  02-2015  01-2016  01-2017  02-2018  02-2019  02-2020 
+1 (01-09)  01-1993  01-1994  02-1995  02-1996  02-1997  02-1998  01-1999  01-2000  02-2001  02-2002  02-2003  02-2004  01-2005  02-2006  02-2007  02-2008  02-2009  01-2010  01-2011  02-2012  02-2013  02-2014  02-2015  01-2016  02-2017  02-2018  02-2019  02-2020 
+1 (01-10)  01-1993  02-1994  02-1995  02-1996  02-1997  02-1998  01-1999  02-2000  02-2001  02-2002  02-2003  02-2004  02-2005  02-2006  02-2007  02-2008  02-2009  01-2010  02-2011  02-2012  02-2013  02-2014  02-2015  01-2016  02-2017  02-2018  02-2019  02-2020 
+1 (01-11)  02-1993  02-1994  02-1995  02-1996  02-1997  02-1998  02-1999  02-2000  02-2001  02-2002  02-2003  02-2004  02-2005  02-2006  02-2007  02-2008  02-2009  02-2010  02-2011  02-2012  02-2013  02-2014  02-2015  02-2016  02-2017  02-2018  02-2019  02-2020 
+1 (01-12)  02-1993  02-1994  02-1995  02-1996  02-1997  03-1998  02-1999  02-2000  02-2001  02-2002  02-2003  03-2004  02-2005  02-2006  02-2007  02-2008  03-2009  02-2010  02-2011  02-2012  02-2013  02-2014  03-2015  02-2016  02-2017  02-2018  02-2019  02-2020 
+1 (01-13)  02-1993  02-1994  02-1995  02-1996  03-1997  03-1998  02-1999  02-2000  02-2001  02-2002  03-2003  03-2004  02-2005  02-2006  02-2007  02-2008  03-2009  02-2010  02-2011  02-2012  02-2013  03-2014  03-2015  02-2016  02-2017  02-2018  02-2019  03-2020 
+1 (01-14)  02-1993  02-1994  02-1995  02-1996  03-1997  03-1998  02-1999  02-2000  02-2001  03-2002  03-2003  03-2004  02-2005  02-2006  02-2007  03-2008  03-2009  02-2010  02-2011  02-2012  03-2013  03-2014  03-2015  02-2016  02-2017  02-2018  03-2019  03-2020 
+1 (01-15)  02-1993  02-1994  02-1995  03-1996  03-1997  03-1998  02-1999  02-2000  03-2001  03-2002  03-2003  03-2004  02-2005  02-2006  03-2007  03-2008  03-2009  02-2010  02-2011  02-2012  03-2013  03-2014  03-2015  02-2016  02-2017  03-2018  03-2019  03-2020 
+1 (01-16)  02-1993  02-1994  03-1995  03-1996  03-1997  03-1998  02-1999  02-2000  03-2001  03-2002  03-2003  03-2004  02-2005  03-2006  03-2007  03-2008  03-2009  02-2010  02-2011  03-2012  03-2013  03-2014  03-2015  02-2016  03-2017  03-2018  03-2019  03-2020 
+1 (01-17)  02-1993  03-1994  03-1995  03-1996  03-1997  03-1998  02-1999  03-2000  03-2001  03-2002  03-2003  03-2004  03-2005  03-2006  03-2007  03-2008  03-2009  02-2010  03-2011  03-2012  03-2013  03-2014  03-2015  02-2016  03-2017  03-2018  03-2019  03-2020 
+1 (01-18)  03-1993  03-1994  03-1995  03-1996  03-1997  03-1998  03-1999  03-2000  03-2001  03-2002  03-2003  03-2004  03-2005  03-2006  03-2007  03-2008  03-2009  03-2010  03-2011  03-2012  03-2013  03-2014  03-2015  03-2016  03-2017  03-2018  03-2019  03-2020 
----"#;

/// Expected output of `bug37514.phpt`.
const EXP_BUG37514: &str = r#"Fri, 18 May 2007 05:05:00 +0000
Fri, 18 May 2007 17:05:00 +0000
Fri, 18 May 2007 17:05:00 +0000
Fri, 18 May 2007 05:05:00 +0000
Fri, 18 May 2007 05:05:00 +0000
Thu, 18 May 2006 17:05:00 +0000"#;

/// Expected output of `bug61642.phpt`.
const EXP_BUG61642: &str = r#"###  2012-03-29 Thu  2012-03-30 Fri  2012-03-31 Sat  2012-04-01 Sun  2012-04-02 Mon  2012-04-03 Tue  2012-04-04 Wed  2012-04-05 Thu

-11  2012-03-14 Wed  2012-03-15 Thu  2012-03-16 Fri  2012-03-16 Fri  2012-03-16 Fri  2012-03-19 Mon  2012-03-20 Tue  2012-03-21 Wed
-10  2012-03-15 Thu  2012-03-16 Fri  2012-03-19 Mon  2012-03-19 Mon  2012-03-19 Mon  2012-03-20 Tue  2012-03-21 Wed  2012-03-22 Thu
 -9  2012-03-16 Fri  2012-03-19 Mon  2012-03-20 Tue  2012-03-20 Tue  2012-03-20 Tue  2012-03-21 Wed  2012-03-22 Thu  2012-03-23 Fri
 -8  2012-03-19 Mon  2012-03-20 Tue  2012-03-21 Wed  2012-03-21 Wed  2012-03-21 Wed  2012-03-22 Thu  2012-03-23 Fri  2012-03-26 Mon
 -7  2012-03-20 Tue  2012-03-21 Wed  2012-03-22 Thu  2012-03-22 Thu  2012-03-22 Thu  2012-03-23 Fri  2012-03-26 Mon  2012-03-27 Tue
 -6  2012-03-21 Wed  2012-03-22 Thu  2012-03-23 Fri  2012-03-23 Fri  2012-03-23 Fri  2012-03-26 Mon  2012-03-27 Tue  2012-03-28 Wed
 -5  2012-03-22 Thu  2012-03-23 Fri  2012-03-26 Mon  2012-03-26 Mon  2012-03-26 Mon  2012-03-27 Tue  2012-03-28 Wed  2012-03-29 Thu
 -4  2012-03-23 Fri  2012-03-26 Mon  2012-03-27 Tue  2012-03-27 Tue  2012-03-27 Tue  2012-03-28 Wed  2012-03-29 Thu  2012-03-30 Fri
 -3  2012-03-26 Mon  2012-03-27 Tue  2012-03-28 Wed  2012-03-28 Wed  2012-03-28 Wed  2012-03-29 Thu  2012-03-30 Fri  2012-04-02 Mon
 -2  2012-03-27 Tue  2012-03-28 Wed  2012-03-29 Thu  2012-03-29 Thu  2012-03-29 Thu  2012-03-30 Fri  2012-04-02 Mon  2012-04-03 Tue
 -1  2012-03-28 Wed  2012-03-29 Thu  2012-03-30 Fri  2012-03-30 Fri  2012-03-30 Fri  2012-04-02 Mon  2012-04-03 Tue  2012-04-04 Wed
 +0  2012-03-29 Thu  2012-03-30 Fri  2012-04-02 Mon  2012-04-02 Mon  2012-04-02 Mon  2012-04-03 Tue  2012-04-04 Wed  2012-04-05 Thu
 +1  2012-03-30 Fri  2012-04-02 Mon  2012-04-02 Mon  2012-04-02 Mon  2012-04-03 Tue  2012-04-04 Wed  2012-04-05 Thu  2012-04-06 Fri
 +2  2012-04-02 Mon  2012-04-03 Tue  2012-04-03 Tue  2012-04-03 Tue  2012-04-04 Wed  2012-04-05 Thu  2012-04-06 Fri  2012-04-09 Mon
 +3  2012-04-03 Tue  2012-04-04 Wed  2012-04-04 Wed  2012-04-04 Wed  2012-04-05 Thu  2012-04-06 Fri  2012-04-09 Mon  2012-04-10 Tue
 +4  2012-04-04 Wed  2012-04-05 Thu  2012-04-05 Thu  2012-04-05 Thu  2012-04-06 Fri  2012-04-09 Mon  2012-04-10 Tue  2012-04-11 Wed
 +5  2012-04-05 Thu  2012-04-06 Fri  2012-04-06 Fri  2012-04-06 Fri  2012-04-09 Mon  2012-04-10 Tue  2012-04-11 Wed  2012-04-12 Thu
 +6  2012-04-06 Fri  2012-04-09 Mon  2012-04-09 Mon  2012-04-09 Mon  2012-04-10 Tue  2012-04-11 Wed  2012-04-12 Thu  2012-04-13 Fri
 +7  2012-04-09 Mon  2012-04-10 Tue  2012-04-10 Tue  2012-04-10 Tue  2012-04-11 Wed  2012-04-12 Thu  2012-04-13 Fri  2012-04-16 Mon
 +8  2012-04-10 Tue  2012-04-11 Wed  2012-04-11 Wed  2012-04-11 Wed  2012-04-12 Thu  2012-04-13 Fri  2012-04-16 Mon  2012-04-17 Tue
 +9  2012-04-11 Wed  2012-04-12 Thu  2012-04-12 Thu  2012-04-12 Thu  2012-04-13 Fri  2012-04-16 Mon  2012-04-17 Tue  2012-04-18 Wed
+10  2012-04-12 Thu  2012-04-13 Fri  2012-04-13 Fri  2012-04-13 Fri  2012-04-16 Mon  2012-04-17 Tue  2012-04-18 Wed  2012-04-19 Thu
+11  2012-04-13 Fri  2012-04-16 Mon  2012-04-16 Mon  2012-04-16 Mon  2012-04-17 Tue  2012-04-18 Wed  2012-04-19 Thu  2012-04-20 Fri"#;

/// Expected output of `bug67253.phpt`.
const EXP_BUG67253: &str = r#"array(12) {
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
  int(3)
  ["errors"]=>
  array(2) {
    [0]=>
    string(29) "A meridian could not be found"
    [9]=>
    string(43) "Not enough data available to satisfy format"
  }
  ["is_localtime"]=>
  bool(false)
}"#;

/// Expected output of `bug68549-dst-transition-DateTime-setTimestamp.phpt`.
const EXP_BUG68549_DST_TRANSITION_DATETIME_SETTIMESTAMP: &str = r#"America/Los_Angeles 1615687199 -> 1615687199 2021-03-13T17:59:59-0800
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
Europe/Amsterdam 1635649201 -> 1635649201 2021-10-31T04:00:01+0100"#;

/// Expected output of `bug78452.phpt`.
const EXP_BUG78452: &str = r#"object(DateInterval)#3 (%d) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(1)
  ["d"]=>
  int(2)
  ["h"]=>
  int(23)
  ["i"]=>
  int(0)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(1)
  ["days"]=>
  int(33)
  ["from_string"]=>
  bool(false)
}"#;

/// Expected output of `bug81263.phpt`.
const EXP_BUG81263: &str = r#"DateInterval Object
(
    [y] => 0
    [m] => 0
    [d] => 0
    [h] => 0
    [i] => 0
    [s] => 0
    [f] => 0
    [invert] => 0
    [days] => 0
    [from_string] => 
)
DateInterval Object
(
    [y] => 0
    [m] => 0
    [d] => 0
    [h] => 0
    [i] => 0
    [s] => 0
    [f] => 0
    [invert] => 0
    [days] => 0
    [from_string] => 
)"#;

/// Expected output of `date-parse-by-format001.phpt`.
const EXP_DATE_PARSE_BY_FORMAT001: &str = r#"Array
(
    [year] => 2004
    [month] => 6
    [day] => 8
    [hour] => 0
    [minute] => 0
    [second] => 0
    [fraction] => 0
    [warning_count] => 0
    [warnings] => Array
        (
        )

    [error_count] => 0
    [errors] => Array
        (
        )

    [is_localtime] => 
)
Array
(
    [year] => 2004
    [month] => 6
    [day] => 8
    [hour] => 0
    [minute] => 0
    [second] => 0
    [fraction] => 0
    [warning_count] => 0
    [warnings] => Array
        (
        )

    [error_count] => 0
    [errors] => Array
        (
        )

    [is_localtime] => 
)"#;

/// Expected output of `getdate_variation4.phpt`.
const EXP_GETDATE_VARIATION4: &str = r#"*** Testing getdate() : usage variation ***

--01 Jan 1970--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(30)
  ["hours"]=>
  int(5)
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

--01 Jan 1971--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(30)
  ["hours"]=>
  int(5)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(5)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1971)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(6) "Friday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(31536000)
}

--01 Jan 1972--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(30)
  ["hours"]=>
  int(5)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(6)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1972)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(8) "Saturday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(63072000)
}

--01 Jan 1973--
array(11) {
  ["seconds"]=>
  int(0)
  ["minutes"]=>
  int(30)
  ["hours"]=>
  int(5)
  ["mday"]=>
  int(1)
  ["wday"]=>
  int(1)
  ["mon"]=>
  int(1)
  ["year"]=>
  int(1973)
  ["yday"]=>
  int(0)
  ["weekday"]=>
  string(6) "Monday"
  ["month"]=>
  string(7) "January"
  [0]=>
  int(94694400)
}"#;

/// Expected output of `mktime-3-64bit.phpt`.
const EXP_MKTIME_3_64BIT: &str = r#"America/Toronto
Y:    0 - January 2000-01-01T01:01:01-0500
Y:   69 - January 2069-01-01T01:01:01-0500
Y:   70 - January 1970-01-01T01:01:01-0500
Y:   71 - January 1971-01-01T01:01:01-0500
Y:   99 - January 1999-01-01T01:01:01-0500
Y:  100 - January 2000-01-01T01:01:01-0500
Y:  101 - January 0101-01-01T01:01:01-0%d
Y:  105 - January 0105-01-01T01:01:01-0%d
Y:  110 - January 0110-01-01T01:01:01-0%d
Y: 1900 - January 1900-01-01T01:01:01-0%d
Y: 1901 - January 1901-01-01T01:01:01-0%d
Y: 1902 - January 1902-01-01T01:01:01-0500
Y: 1999 - January 1999-01-01T01:01:01-0500
Y: 2000 - January 2000-01-01T01:01:01-0500
Y: 2001 - January 2001-01-01T01:01:01-0500

Europe/Oslo
Y:    0 - January 2000-01-01T01:01:01+0100
Y:   69 - January 2069-01-01T01:01:01+0100
Y:   70 - January 1970-01-01T01:01:01+0100
Y:   71 - January 1971-01-01T01:01:01+0100
Y:   99 - January 1999-01-01T01:01:01+0100
Y:  100 - January 2000-01-01T01:01:01+0100
Y:  101 - January 0101-01-01T01:01:01+0%d
Y:  105 - January 0105-01-01T01:01:01+0%d
Y:  110 - January 0110-01-01T01:01:01+0%d
Y: 1900 - January 1900-01-01T01:01:01+0%d
Y: 1901 - January 1901-01-01T01:01:01+0%d
Y: 1902 - January 1902-01-01T01:01:01+0100
Y: 1999 - January 1999-01-01T01:01:01+0100
Y: 2000 - January 2000-01-01T01:01:01+0100
Y: 2001 - January 2001-01-01T01:01:01+0100"#;

/// Expected output of `rfc-datetime_and_daylight_saving_time-type3-bd2.phpt`.
const EXP_RFC_DATETIME_AND_DAYLIGHT_SAVING_TIME_TYPE3_BD2: &str = r#"bd0 2010-11-07 01:00:00 EST America/New_York - 2010-11-07 01:59:59 EDT America/New_York = P0DT0H0M1S
bd5 2010-11-07 01:30:00 EST America/New_York - 2010-11-06 04:30:00 EDT America/New_York = P0DT22H
bd6 2010-11-07 01:30:00 EDT America/New_York - 2010-11-06 04:30:00 EDT America/New_York = P0DT21H
bd8a 2010-11-07 01:00:00 EST America/New_York - 2010-11-06 01:00:00 EDT America/New_York = P1DT0H
bd8b 2010-11-07 01:00:00 EDT America/New_York - 2010-11-06 01:00:00 EDT America/New_York = P0DT24H"#;

/// Expected output of `test-parse-from-format.phpt`.
const EXP_TEST_PARSE_FROM_FORMAT: &str = r#"string(13) "Y-m-d\TH:i:sP"
string(25) "2008-07-08T22:14:12+02:00"
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(16) "l, d-M-Y H:i:s T"
string(38) "Tuesday, 08-Jul-2008 22:14:12 GMT+0200"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(13) "Y-m-d\TH:i:sO"
string(24) "2008-07-08T22:14:12+0200"
object(DateTime)#3 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(13) "X-m-d\TH:i:sP"
string(26) "+2008-07-08T22:14:12+02:00"
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(16) "D, d M y H:i:s O"
string(29) "Tue, 08 Jul 08 22:14:12 +0200"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(16) "l, d-M-y H:i:s T"
string(36) "Tuesday, 08-Jul-08 22:14:12 GMT+0200"
object(DateTime)#3 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(16) "D, d M y H:i:s O"
string(29) "Tue, 08 Jul 08 22:14:12 +0200"
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(16) "D, d M Y H:i:s O"
string(31) "Tue, 08 Jul 2008 22:14:12 +0200"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(16) "D, d M Y H:i:s O"
string(31) "Tue, 08 Jul 2008 22:14:12 +0200"
object(DateTime)#3 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(13) "Y-m-d\TH:i:sP"
string(25) "2008-07-08T22:14:12+02:00"
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(15) "Y-m-d\TH:i:s.vP"
string(29) "2008-07-08T22:14:12.000+02:00"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(16) "D, d M Y H:i:s O"
string(31) "Tue, 08 Jul 2008 22:14:12 +0200"
object(DateTime)#3 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}

string(13) "Y-m-d\TH:i:sP"
string(25) "2008-07-08T22:14:12+02:00"
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2008-07-08 22:14:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}"#;

/// Expected output of `bug77243-001.phpt`.
const EXP_BUG77243_001: &str = r#"-0001-12-24 -1-51-5
-0001-12-25 -1-51-6
-0001-12-26 -1-51-7
-0001-12-27 -1-52-1
-0001-12-28 -1-52-2
-0001-12-29 -1-52-3
-0001-12-30 -1-52-4
-0001-12-31 -1-52-5
0000-01-01 -1-52-6
0000-01-02 -1-52-7
0000-01-03 0-01-1
0000-01-04 0-01-2
0000-01-05 0-01-3
0000-01-06 0-01-4
0000-01-07 0-01-5
0000-01-08 0-01-6
0000-01-09 0-01-7
0000-01-10 0-02-1
0000-01-11 0-02-2
0000-01-12 0-02-3"#;

/// Expected output of `date_sunrise_variation7.phpt`.
const EXP_DATE_SUNRISE_VARIATION7: &str = r#"*** Testing date_sunrise() : usage variation ***
string(5) "06:42"
string(5) "06:41"
string(5) "05:07"
string(5) "05:09"
string(5) "05:58"
string(5) "05:59"
string(5) "07:31"
string(5) "07:30"
string(5) "05:52"
string(5) "05:53"
string(5) "05:59"
string(5) "06:01"
string(5) "06:01"
string(5) "06:02"
string(5) "06:23"
string(5) "06:22"
string(5) "06:03"
string(5) "06:02""#;

/// Expected output of `gmdate_variation12_64bits.phpt`.
const EXP_GMDATE_VARIATION12_64BITS: &str = r#"*** Testing gmdate() : usage variation ***

-- Testing gmdate() function with minimum range of timestamp --
string(24) "1901-12-13T20:45:54+0000"

-- Testing gmdate() function with less than the range of timestamp --
string(24) "1901-12-13T20:45:50+0000"

-- Testing gmdate() function with maximum range of timestamp --
string(24) "2038-01-19T03:14:07+0000"

-- Testing gmdate() function with greater than the range of timestamp --
string(24) "2038-01-19T03:14:10+0000""#;

/// Expected output of `date_sub_basic.phpt`.
const EXP_DATE_SUB_BASIC: &str = r#"*** Testing date_sub() : basic functionality ***
string(19) "2008-01-01 12:25:00"
string(19) "2004-06-26 23:54:55"
string(19) "2004-06-26 23:54:55"
string(19) "2004-06-24 23:53:55"
string(19) "2003-04-24 00:08:25""#;

/// Expected output of `bug52290.phpt`.
const EXP_BUG52290: &str = r#"string(47) "2005-W52-7 | 2006-01-01 | 00:00:00 | 1136073600"
string(47) "2005-W52-1 | 2005-12-26 | 00:00:00 | 1135555200"
string(47) "2007-W41-3 | 2007-10-10 | 00:00:00 | 1191974400"
string(47) "2007-W41-3 | 2007-10-10 | 20:30:40 | 1192048240""#;

/// Expected output of `bug52668.phpt`.
const EXP_BUG52668: &str = r#"Sun, 12 Dec 2010 00:00:00 +0000
Mon, 13 Dec 2010 00:00:00 +0000
Sun, 12 Dec 2010 00:00:00 +0000
Sun, 12 Dec 2010 00:00:00 +0000
Mon, 13 Dec 2010 00:00:00 +0000
Sun, 12 Dec 2010 00:00:00 +0000"#;

/// Expected output of `DateTimeZone_getLocation.phpt`.
const EXP_DATETIMEZONE_GETLOCATION: &str = r#"Array
(
    [country_code] => %s
    [latitude] => %f
    [longitude] => %f
    [comments] => %s
)
Array
(
    [country_code] => %s
    [latitude] => %f
    [longitude] => %f
    [comments] => %s
)
Array
(
    [country_code] => %s
    [latitude] => %f
    [longitude] => %f
    [comments] => %s
)
Array
(
    [country_code] => %s
    [latitude] => %f
    [longitude] => %f
    [comments] => %S
)"#;

/// Expected output of `bug51987.phpt`.
const EXP_BUG51987: &str = r#"object(DateTime)#%d (%d) {
  ["date"]=>
  string(26) "1985-04-12 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}"#;

/// Expected output of `bug79716.phpt`.
const EXP_BUG79716: &str = r#"bool(false)
object(DateTimeImmutable)#%d (%d) {
  ["date"]=>
  string(26) "2769-12-31 15:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#;

/// Expected output of `strtotime3.phpt`.
const EXP_STRTOTIME3: &str = r#"bool(false)
bool(false)
string(31) "Thu, 15 Jun 2006 00:00:00 +0100"
string(31) "Fri, 16 Jun 2006 22:49:12 +0100"
bool(false)
string(31) "Fri, 16 Jun 2006 23:49:12 +0100"
bool(false)
string(31) "Fri, 16 Jun 2006 02:22:00 +0100"

Warning: strtotime(): Epoch doesn't fit in a PHP integer in %s on line %d
bool(false)
string(31) "Fri, 16 Jun 2006 02:22:33 +0100"
bool(false)
string(31) "Tue, 02 Mar 2004 00:00:00 +0000"
string(31) "Tue, 02 Mar 2004 00:00:00 +0000"
string(31) "Sun, 12 Feb 2006 23:12:23 +0000"
bool(false)
string(31) "Fri, 16 Jun 2006 00:00:00 +0100"
string(31) "Sun, 15 Jan 2006 00:00:00 +0000"
string(31) "Sun, 15 Jan 2006 00:00:00 +0000"
string(31) "Tue, 10 Oct 2000 13:55:36 +0100"
bool(false)
string(31) "Fri, 16 Jun 2006 20:06:00 +0100"
string(31) "Mon, 16 Jun 1986 22:51:59 +0100"
string(31) "Mon, 16 Jan 2006 00:00:00 +0000"
string(31) "Mon, 16 Jan 2006 00:00:00 +0000""#;

/// Expected output of `date_constants.phpt`.
const EXP_DATE_CONSTANTS: &str = r#"Deprecated: Constant DATE_RFC7231 is deprecated since 8.5, as this format ignores the associated timezone and always uses GMT in %s on line %d
string(25) "2006-07-01T14:27:30+02:00"
string(25) "2006-05-30T14:32:13+02:00"
string(35) "Saturday, 01-Jul-2006 14:27:30 CEST"
string(34) "Tuesday, 30-May-2006 14:32:13 CEST"
string(24) "2006-07-01T14:27:30+0200"
string(24) "2006-05-30T14:32:13+0200"
string(26) "+2006-07-01T14:27:30+02:00"
string(26) "+2006-05-30T14:32:13+02:00"
string(29) "Sat, 01 Jul 06 14:27:30 +0200"
string(29) "Tue, 30 May 06 14:32:13 +0200"
string(33) "Saturday, 01-Jul-06 14:27:30 CEST"
string(32) "Tuesday, 30-May-06 14:32:13 CEST"
string(29) "Sat, 01 Jul 06 14:27:30 +0200"
string(29) "Tue, 30 May 06 14:32:13 +0200"
string(31) "Sat, 01 Jul 2006 14:27:30 +0200"
string(31) "Tue, 30 May 2006 14:32:13 +0200"
string(29) "Sat, 01 Jul 2006 14:27:30 GMT"
string(29) "Tue, 30 May 2006 14:32:13 GMT"
string(31) "Sat, 01 Jul 2006 14:27:30 +0200"
string(31) "Tue, 30 May 2006 14:32:13 +0200"
string(25) "2006-07-01T14:27:30+02:00"
string(25) "2006-05-30T14:32:13+02:00"
string(29) "2006-07-01T14:27:30.000+02:00"
string(29) "2006-05-30T14:32:13.000+02:00"
string(31) "Sat, 01 Jul 2006 14:27:30 +0200"
string(31) "Tue, 30 May 2006 14:32:13 +0200"
string(25) "2006-07-01T14:27:30+02:00"
string(25) "2006-05-30T14:32:13+02:00"


Deprecated: Constant DATE_RFC7231 is deprecated since 8.5, as this format ignores the associated timezone and always uses GMT in %s on line %d

Deprecated: Constant DateTimeInterface::RFC7231 is deprecated since 8.5, as this format ignores the associated timezone and always uses GMT in %s on line %d
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)"#;

/// Expected output of `bug80913.phpt`.
const EXP_BUG80913: &str = r#"string(25) "2021-03-28T03:00:00+02:00"
string(25) "2021-03-28T01:30:00+01:00"
string(25) "2021-03-28T03:30:00+02:00""#;

/// Expected output of `bug69336.phpt`.
const EXP_BUG69336: &str = r#"string(10) "30.04.%d"
string(10) "31.03.2015"
string(10) "25.03.2015"
string(10) "29.04.2015"
string(10) "26.03.2014"
string(10) "30.04.2014""#;

/// Expected output of `bug35624.phpt`.
const EXP_BUG35624: &str = r#"MonMon
MonMon
TueTue
TueTue
WedWed
WedWed
ThuThu
ThuThu
FriFri
FriFri
SatSat
SatSat
SunSun
SunSun"#;

/// Expected output of `bug41844.phpt`.
const EXP_BUG41844: &str = r#"-0999-06-28T00:00:00+0000
-0001-06-28T00:00:00+0000"#;

/// Expected output of `bug33056.phpt`.
const EXP_BUG33056: &str = r#"1116406800
1116407554
1116443554"#;

/// Expected output of `strtotime-mysql-64bit.phpt`.
const EXP_STRTOTIME_MYSQL_64BIT: &str = r#"string(31) "Fri, 23 May 1997 09:15:28 +0000"
string(31) "Sun, 31 Dec 2000 18:58:59 +0000"
string(31) "Wed, 10 Apr 2080 10:10:10 +0000""#;

/// Expected output of `007.phpt`.
const EXP_007: &str = r#"array(9) {
  [0]=>
  int(%d)
  [1]=>
  int(%d)
  [2]=>
  int(%d)
  [3]=>
  int(%d)
  [4]=>
  int(%d)
  [5]=>
  int(%d)
  [6]=>
  int(%d)
  [7]=>
  int(%d)
  [8]=>
  int(%d)
}
array(9) {
  [0]=>
  int(0)
  [1]=>
  int(0)
  [2]=>
  int(0)
  [3]=>
  int(27)
  [4]=>
  int(5)
  [5]=>
  int(106)
  [6]=>
  int(2)
  [7]=>
  int(177)
  [8]=>
  int(0)
}
array(9) {
  ["tm_sec"]=>
  int(0)
  ["tm_min"]=>
  int(0)
  ["tm_hour"]=>
  int(0)
  ["tm_mday"]=>
  int(27)
  ["tm_mon"]=>
  int(5)
  ["tm_year"]=>
  int(106)
  ["tm_wday"]=>
  int(2)
  ["tm_yday"]=>
  int(177)
  ["tm_isdst"]=>
  int(0)
}
array(9) {
  [0]=>
  int(0)
  [1]=>
  int(0)
  [2]=>
  int(0)
  [3]=>
  int(27)
  [4]=>
  int(5)
  [5]=>
  int(106)
  [6]=>
  int(2)
  [7]=>
  int(177)
  [8]=>
  int(0)
}
Done"#;

/// Expected output of `DateTimeZone_listAbbreviations_basic1.phpt`.
const EXP_DATETIMEZONE_LISTABBREVIATIONS_BASIC1: &str = r#"*** Testing DateTimeZone::listAbbreviations() : basic functionality ***
string(5) "array"
int(144)

-- Format a sample entry --
array(6) {
  [0]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(34200)
    ["timezone_id"]=>
    string(18) "Australia/Adelaide"
  }
  [1]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(34200)
    ["timezone_id"]=>
    string(21) "Australia/Broken_Hill"
  }
  [2]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(34200)
    ["timezone_id"]=>
    string(16) "Australia/Darwin"
  }
  [3]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(34200)
    ["timezone_id"]=>
    string(15) "Australia/North"
  }
  [4]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(34200)
    ["timezone_id"]=>
    string(15) "Australia/South"
  }
  [5]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(34200)
    ["timezone_id"]=>
    string(20) "Australia/Yancowinna"
  }
}"#;

/// Expected output of `date_period_bad_iso_format.phpt`.
const EXP_DATE_PERIOD_BAD_ISO_FORMAT: &str = r#"Deprecated: Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead in %s on line %d
DateMalformedPeriodStringException: DatePeriod::__construct(): ISO interval must contain a start date, "R4" given
DateMalformedPeriodStringException: DatePeriod::createFromISO8601String(): ISO interval must contain a start date, "R4" given

Deprecated: Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead in %s on line %d
DateMalformedPeriodStringException: DatePeriod::__construct(): ISO interval must contain an interval, "R4/2012-07-01T00:00:00Z" given
DateMalformedPeriodStringException: DatePeriod::createFromISO8601String(): ISO interval must contain an interval, "R4/2012-07-01T00:00:00Z" given

Deprecated: Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead in %s on line %d
DateMalformedPeriodStringException: DatePeriod::__construct(): ISO interval must contain an end date or a recurrence count, "2012-07-01T00:00:00Z/P7D" given
DateMalformedPeriodStringException: DatePeriod::createFromISO8601String(): ISO interval must contain an end date or a recurrence count, "2012-07-01T00:00:00Z/P7D" given"#;

/// Expected output of `DatePeriod_createFromISO8601String_static_return.phpt`.
const EXP_DATEPERIOD_CREATEFROMISO8601STRING_STATIC_RETURN: &str = r#"object(MyDatePeriod)#1 (7) {
  ["start"]=>
  object(DateTimeImmutable)#2 (3) {
    ["date"]=>
    string(26) "2012-07-01 00:00:00.000000"
    ["timezone_type"]=>
    int(1)
    ["timezone"]=>
    string(6) "+00:00"
  }
  ["current"]=>
  NULL
  ["end"]=>
  NULL
  ["interval"]=>
  object(DateInterval)#3 (10) {
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
  ["recurrences"]=>
  int(5)
  ["include_start_date"]=>
  bool(true)
  ["include_end_date"]=>
  bool(false)
}
DateMalformedPeriodStringException: Unknown or bad format (R4/2012-07-01T00:/P7D)
DateMalformedPeriodStringException: DatePeriod::createFromISO8601String(): ISO interval must contain an interval, "R4/2012-07-01T00:00:00Z" given"#;

/// Expected output of `date_interval_bad_format_leak.phpt`.
const EXP_DATE_INTERVAL_BAD_FORMAT_LEAK: &str = r#"DateMalformedIntervalStringException: Unknown or bad format (P3"D)

Deprecated: Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead in %s on line %d
DateMalformedPeriodStringException: Unknown or bad format (P3"D)
DateMalformedPeriodStringException: Unknown or bad format (P3"D)

Deprecated: Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead in %s on line %d
DateMalformedPeriodStringException: Unknown or bad format (2008-03-01T12:00:00Z1)
DateMalformedPeriodStringException: Unknown or bad format (2008-03-01T12:00:00Z1)"#;

/// Expected output of `timezone_open_warning.phpt`.
const EXP_TIMEZONE_OPEN_WARNING: &str = r#"In: +02:30; Out: +02:30
In: Europe/Kyiv; Out: Europe/Kyiv

Warning: timezone_open(): Unknown or bad timezone (2.5) in %stimezone_open_warning.php on line 6

Warning: timezone_open(): Unknown or bad timezone (99:60) in %stimezone_open_warning.php on line 6

Warning: timezone_open(): Unknown or bad timezone (Europe/Lviv) in %stimezone_open_warning.php on line 6"#;

/// Expected output of `gmdate_variation7.phpt`.
const EXP_GMDATE_VARIATION7: &str = r#"*** Testing gmdate() : usage variation ***

-- Testing gmdate() function with checking non leap year using Leap Year format --
string(1) "0"

-- Testing gmdate() function with checking leap year using Leap Year format --
string(1) "%d"
string(1) "1"

-- Testing gmdate() function with ISO-8601 year number format --
string(4) "%d"
string(4) "2008"

-- Testing gmdate() function with full numeric representation of year format --
string(4) "%d"
string(4) "2008"

-- Testing gmdate() function with 2 digit representation year format --
string(2) "%d"
string(2) "08""#;

/// Expected output of `oo_002.phpt`.
const EXP_OO_002: &str = r#"string(29) "Wed, 01 Aug 07 13:00:00 +0000"
string(29) "Wed, 01 Aug 07 13:00:00 +0000"
string(29) "Wed, 01 Aug 07 14:00:00 +0000"
string(29) "Wed, 01 Aug 07 12:59:59 +0000"
string(10) "Asia/Tokyo"
string(10) "Asia/Tokyo""#;
