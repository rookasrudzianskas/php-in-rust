//! Tests ported from php-src `ext/date/tests/*.phpt` (group 4).
//!
//! Each test names the phpt file it was ported from. Partial ports say what was left
//! out (PHP-engine behaviour such as serialization, exceptions or object handles).
mod common;

use std::fmt::Write as _;

use common::*;
use php_timelib::php::{self, PhpTimeZone};
use php_timelib::{RelTime, Time, ZoneType, builtin_tzinfo};

const DATE_ISO8601: &str = "Y-m-d\\TH:i:sO";
const DATE_RFC822: &str = "D, d M y H:i:s O";
const DATE_RFC2822: &str = "D, d M Y H:i:s O";
const DATE_COOKIE: &str = "l, d-M-Y H:i:s T";
const DATE_RFC3339_EXTENDED: &str = "Y-m-d\\TH:i:s.vP";

// ---------------------------------------------------------------------------------
// Helpers

/// A minimal version of run-tests' EXPECTF matching (`%s %S %a %A %d %i %x %w %c %%`).
fn expectf_match(mut a: &[u8], mut p: &[u8]) -> bool {
    fn line(b: u8) -> bool {
        b != b'\n' && b != b'\r'
    }
    fn any(_: u8) -> bool {
        true
    }
    fn digit(b: u8) -> bool {
        b.is_ascii_digit()
    }
    fn hex(b: u8) -> bool {
        b.is_ascii_hexdigit()
    }
    fn space(b: u8) -> bool {
        b.is_ascii_whitespace()
    }
    loop {
        match p {
            [] => return a.is_empty(),
            [b'%', b'%', rest @ ..] => {
                if a.first() != Some(&b'%') {
                    return false;
                }
                a = &a[1..];
                p = rest;
            }
            [b'%', b'c', rest @ ..] => {
                if a.is_empty() {
                    return false;
                }
                a = &a[1..];
                p = rest;
            }
            [
                b'%',
                c @ (b's' | b'S' | b'a' | b'A' | b'd' | b'i' | b'x' | b'w'),
                rest @ ..,
            ] => {
                let (min, class): (usize, fn(u8) -> bool) = match c {
                    b's' => (1, line),
                    b'S' => (0, line),
                    b'a' => (1, any),
                    b'A' => (0, any),
                    b'x' => (1, hex),
                    b'w' => (0, space),
                    _ => (1, digit),
                };
                let start = usize::from(*c == b'i' && matches!(a.first(), Some(b'+' | b'-')));
                let mut n = start;
                while n < a.len() && class(a[n]) {
                    n += 1;
                }
                if n - start < min {
                    return false;
                }
                return (start + min..=n)
                    .rev()
                    .any(|k| expectf_match(&a[k..], rest));
            }
            [x, rest @ ..] => {
                if a.first() != Some(x) {
                    return false;
                }
                a = &a[1..];
                p = rest;
            }
        }
    }
}

/// Asserts that `actual` matches an EXPECTF section.
#[track_caller]
fn assert_expectf(actual: &str, expected: &str) {
    let a = actual.trim_end();
    let e = expected.trim_end();
    if !expectf_match(a.as_bytes(), e.as_bytes()) {
        let mut msg = String::new();
        for (i, (la, le)) in a.lines().zip(e.lines()).enumerate() {
            if !expectf_match(la.as_bytes(), le.as_bytes()) {
                let _ = writeln!(
                    msg,
                    "first mismatching line {}:\n  actual:   {la:?}\n  expected: {le:?}",
                    i + 1
                );
                break;
            }
        }
        panic!("EXPECTF mismatch\n{msg}--- actual ---\n{a}\n--- expected ---\n{e}");
    }
}

fn dump_str(s: &str) -> String {
    V::Str(s.to_string()).var_dump()
}

fn dump_bool(b: bool) -> String {
    V::Bool(b).var_dump()
}

fn dump_int(i: i64) -> String {
    V::Int(i).var_dump()
}

/// The properties of a `DateTimeZone` object.
fn tz_props(z: &PhpTimeZone) -> Vec<(K, V)> {
    vec![
        (
            K::S("timezone_type".into()),
            V::Int(i64::from(z.zone_type().as_i32())),
        ),
        (K::S("timezone".into()), V::Str(z.name())),
    ]
}

/// `var_dump($timezone)`.
fn tz_var_dump(z: &PhpTimeZone, id: u32) -> String {
    V::Obj("DateTimeZone".into(), id, tz_props(z)).var_dump()
}

/// `$array[$key]`.
fn v_get<'a>(v: &'a V, k: &K) -> &'a V {
    match v {
        V::Arr(a) | V::Obj(_, _, a) => &a.iter().find(|(kk, _)| kk == k).expect("missing key").1,
        _ => panic!("not an array: {v:?}"),
    }
}

/// `DateTime::getLastErrors()['errors'][$pos]` after a failed `createFromFormat()`.
fn last_error_at(format: &str, s: &str, pos: i64) -> String {
    let errs = last_errors(Some(format), s).expect("no errors");
    v_get(v_get(&errs, &K::S("errors".into())), &K::I(pos)).print_r()
}

/// `php_date_initialize_from_hash()` (what `unserialize()` / `__set_state()` do with
/// the `date`, `timezone_type` and `timezone` properties), for string/int property
/// values. `None` where PHP throws "Invalid serialization data".
fn date_from_hash(date: &str, tz_type: i64, timezone: &str, default_zone: &str) -> Option<DT> {
    match tz_type {
        1 | 2 => DT::try_new(&format!("{date} {timezone}"), None, default_zone).ok(),
        3 => {
            let tzi = builtin_tzinfo(timezone)?;
            DT::try_new(date, Some(&PhpTimeZone::Id(tzi)), default_zone).ok()
        }
        _ => None,
    }
}

/// Round-trips a `DateTime` through its properties (`unserialize(serialize($d))`).
fn date_roundtrip(d: &DT) -> Option<DT> {
    let p = d.props();
    let get = |k: &str| v_get(&V::Arr(p.clone()), &K::S(k.into())).clone();
    let (V::Str(date), V::Int(tz_type), V::Str(tz)) =
        (get("date"), get("timezone_type"), get("timezone"))
    else {
        panic!("unexpected properties {p:?}");
    };
    date_from_hash(&date, tz_type, &tz, &d.default_zone)
}

/// `date_period_advance()`.
fn period_advance(t: &mut Time, interval: &RelTime) {
    t.have_relative = true;
    t.relative = *interval;
    t.sse_uptodate = false;
    t.update_ts(None);
    t.update_from_sse();
}

/// The times `foreach (new DatePeriod($start, $interval, $end, $options))` yields.
fn period_with_end(
    start: &Time,
    interval: &RelTime,
    end: &Time,
    include_start: bool,
    include_end: bool,
) -> Vec<Time> {
    let mut cur = start.clone();
    if !include_start {
        period_advance(&mut cur, interval);
    }
    let mut out = Vec::new();
    loop {
        let more = if cur.sse == end.sse {
            if include_end {
                cur.us <= end.us
            } else {
                cur.us < end.us
            }
        } else {
            cur.sse < end.sse
        };
        if !more {
            break;
        }
        out.push(cur.clone());
        period_advance(&mut cur, interval);
    }
    out
}

/// The `DatePeriod` properties from `start` to `include_end_date` as `var_dump()`
/// shows them inside the period object (`date_period_get_properties`).
fn period_props_dump(
    class: &str,
    start: &DT,
    end: Option<&DT>,
    interval: &php::PhpInterval,
    recurrences: i64,
    include_start: bool,
    include_end: bool,
) -> String {
    let indent = |s: String| s.lines().map(|l| format!("  {l}\n")).collect::<String>();
    let mut out = String::from("  [\"start\"]=>\n");
    out += &indent(start.var_dump(class, 0));
    out += "  [\"current\"]=>\n  NULL\n  [\"end\"]=>\n";
    match end {
        Some(e) => out += &indent(e.var_dump(class, 0)),
        None => out += "  NULL\n",
    }
    out += "  [\"interval\"]=>\n";
    out += &indent(interval_var_dump(interval, 0));
    let _ = write!(
        out,
        "  [\"recurrences\"]=>\n  int({recurrences})\n  [\"include_start_date\"]=>\n  bool({include_start})\n  [\"include_end_date\"]=>\n  bool({include_end})\n"
    );
    out
}

/// `getdate($ts)` with `date_default_timezone_set($zone)`.
fn getdate(ts: i64, zone: &str) -> V {
    let mut t = Time {
        tz_info: Some(tz(zone)),
        zone_type: ZoneType::Id,
        ..Time::new()
    };
    t.unixtime2local(ts);
    let k = |s: &str| K::S(s.to_string());
    V::Arr(vec![
        (k("seconds"), V::Int(t.s)),
        (k("minutes"), V::Int(t.i)),
        (k("hours"), V::Int(t.h)),
        (k("mday"), V::Int(t.d)),
        (k("wday"), V::Int(php_timelib::day_of_week(t.y, t.m, t.d))),
        (k("mon"), V::Int(t.m)),
        (k("year"), V::Int(t.y)),
        (k("yday"), V::Int(php_timelib::day_of_year(t.y, t.m, t.d))),
        (
            k("weekday"),
            V::Str(php_timelib::format::full_day_name(t.y, t.m, t.d).into()),
        ),
        (
            k("month"),
            V::Str(php_timelib::format::MON_FULL_NAMES[(t.m - 1) as usize].into()),
        ),
        (K::I(0), V::Int(ts)),
    ])
}

/// One `examine_diff()` call from the `DateTime_data-*.inc` files: test name, end
/// date, start date, expected spec and the `$absolute` flag.
type Case = (&'static str, &'static str, &'static str, &'static str, bool);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Diff,
    Days,
    Add,
    Sub,
}

/// `examine_diff.inc` run over `cases` with `PHPT_DATETIME_SHOW = show`.
fn examine_diff(cases: &[Case], show: Show, zone: &str) -> String {
    let mut out = String::new();
    for &(name, end_s, start_s, spec, absolute) in cases {
        let mut end = DT::new(end_s, zone);
        let mut start = DT::new(start_s, zone);
        let start_date = start.format("Y-m-d H:i:s T");
        let end_date = end.format("Y-m-d H:i:s T");
        let mut expect_interval = php::date_interval_create(&format!("P{}", &spec[2..])).unwrap();
        if &spec[1..2] == "-" {
            expect_interval.diff.invert = true;
        }
        let line = match show {
            Show::Diff => {
                let r = php::date_diff(&start.t, &end.t, absolute);
                format!(
                    "DIFF: {end_date} - {start_date} = **{}**",
                    interval_format(&r, "P%R%yY%mM%dDT%hH%iM%sS")
                )
            }
            Show::Days => {
                let r = php::date_diff(&start.t, &end.t, absolute);
                format!("DAYS: **{}**", interval_format(&r, "%a"))
            }
            Show::Add => {
                start.add_interval(&expect_interval);
                format!(
                    "ADD: {start_date} + {spec} = **{}**",
                    start.format("Y-m-d H:i:s T")
                )
            }
            Show::Sub => {
                end.sub_interval(&expect_interval);
                format!(
                    "SUB: {end_date} - {spec} = **{}**",
                    end.format("Y-m-d H:i:s T")
                )
            }
        };
        let _ = writeln!(out, "{name}: {line}");
    }
    out
}

const DATA_ABSOLUTE: &[Case] = &[
    (
        "test_absolute_7",
        "2009-01-14",
        "2009-01-07",
        "P+0Y0M7DT0H0M0S",
        true,
    ),
    (
        "test_absolute_negative_7",
        "2009-01-07",
        "2009-01-14",
        "P+0Y0M7DT0H0M0S",
        true,
    ),
];
const DATA_FEBRUARY: &[Case] = &[
    (
        "test_bug_49081__1",
        "2010-03-31",
        "2010-03-01",
        "P+0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__2",
        "2010-04-01",
        "2010-03-01",
        "P+0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__3",
        "2010-04-01",
        "2010-03-31",
        "P+0Y0M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__4",
        "2010-04-29",
        "2010-03-31",
        "P+0Y0M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__5",
        "2010-04-30",
        "2010-03-31",
        "P+0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__6",
        "2010-04-30",
        "2010-03-30",
        "P+0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__7",
        "2010-04-30",
        "2010-03-29",
        "P+0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__8",
        "2010-01-29",
        "2010-01-01",
        "P+0Y0M28DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__9",
        "2010-01-30",
        "2010-01-01",
        "P+0Y0M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__10",
        "2010-01-31",
        "2010-01-01",
        "P+0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__11",
        "2010-02-01",
        "2010-01-01",
        "P+0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__12",
        "2010-02-01",
        "2010-01-31",
        "P+0Y0M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__13",
        "2010-02-27",
        "2010-01-31",
        "P+0Y0M27DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__14",
        "2010-02-28",
        "2010-01-31",
        "P+0Y0M28DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__15",
        "2010-02-28",
        "2010-01-30",
        "P+0Y0M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__16",
        "2010-02-28",
        "2010-01-29",
        "P+0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__17",
        "2010-02-28",
        "2010-01-28",
        "P+0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__18",
        "2010-02-28",
        "2010-01-27",
        "P+0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__19",
        "2010-03-01",
        "2010-01-01",
        "P+0Y2M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__20",
        "2010-03-01",
        "2010-01-31",
        "P+0Y0M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__21",
        "2010-03-27",
        "2010-01-31",
        "P+0Y1M24DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__22",
        "2010-03-28",
        "2010-01-31",
        "P+0Y1M25DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__23",
        "2010-03-29",
        "2010-01-31",
        "P+0Y1M26DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__24",
        "2010-03-30",
        "2010-01-31",
        "P+0Y1M27DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__25",
        "2010-03-31",
        "2010-01-31",
        "P+0Y2M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__26",
        "2010-03-31",
        "2010-01-30",
        "P+0Y2M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__27",
        "2009-01-31",
        "2009-01-01",
        "P+0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__28",
        "2010-03-27",
        "2010-02-28",
        "P+0Y0M27DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__29",
        "2010-03-28",
        "2010-02-28",
        "P+0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__30",
        "2010-03-29",
        "2010-02-28",
        "P+0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__31",
        "2010-03-27",
        "2010-02-27",
        "P+0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081__32",
        "2010-03-27",
        "2010-02-26",
        "P+0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__1",
        "2010-03-01",
        "2010-03-31",
        "P-0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__2",
        "2010-03-01",
        "2010-04-01",
        "P-0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__3",
        "2010-03-31",
        "2010-04-01",
        "P-0Y0M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__4",
        "2010-03-31",
        "2010-04-29",
        "P-0Y0M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__5",
        "2010-03-31",
        "2010-04-30",
        "P-0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__6",
        "2010-03-30",
        "2010-04-30",
        "P-0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__7",
        "2010-03-29",
        "2010-04-30",
        "P-0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__8",
        "2010-01-01",
        "2010-01-29",
        "P-0Y0M28DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__9",
        "2010-01-01",
        "2010-01-30",
        "P-0Y0M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__10",
        "2010-01-01",
        "2010-01-31",
        "P-0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__11",
        "2010-01-01",
        "2010-02-01",
        "P-0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__12",
        "2010-01-31",
        "2010-02-01",
        "P-0Y0M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__13",
        "2010-01-31",
        "2010-02-27",
        "P-0Y0M27DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__14",
        "2010-01-31",
        "2010-02-28",
        "P-0Y0M28DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__15",
        "2010-01-30",
        "2010-02-28",
        "P-0Y0M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__16",
        "2010-01-29",
        "2010-02-28",
        "P-0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__17",
        "2010-01-28",
        "2010-02-28",
        "P-0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__18",
        "2010-01-27",
        "2010-02-28",
        "P-0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__19",
        "2010-01-01",
        "2010-03-01",
        "P-0Y2M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__20",
        "2010-01-31",
        "2010-03-01",
        "P-0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__21",
        "2010-01-31",
        "2010-03-27",
        "P-0Y1M27DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__22",
        "2010-01-31",
        "2010-03-28",
        "P-0Y1M28DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__23",
        "2010-01-31",
        "2010-03-29",
        "P-0Y1M29DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__24",
        "2010-01-31",
        "2010-03-30",
        "P-0Y1M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__25",
        "2010-01-31",
        "2010-03-31",
        "P-0Y2M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__26",
        "2010-01-30",
        "2010-03-31",
        "P-0Y2M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__27",
        "2009-01-01",
        "2009-01-31",
        "P-0Y0M30DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__28",
        "2010-02-28",
        "2010-03-27",
        "P-0Y0M27DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__29",
        "2010-02-28",
        "2010-03-28",
        "P-0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__30",
        "2010-02-28",
        "2010-03-29",
        "P-0Y1M1DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__31",
        "2010-02-27",
        "2010-03-27",
        "P-0Y1M0DT0H0M0S",
        false,
    ),
    (
        "test_bug_49081_negative__32",
        "2010-02-26",
        "2010-03-27",
        "P-0Y1M1DT0H0M0S",
        false,
    ),
];
const DATA_FALL_TYPE2_TYPE2: &[Case] = &[
    (
        "test_time_fall_type2_prev_type2_prev",
        "2010-11-06 18:38:28 EDT",
        "2010-10-04 02:18:48 EDT",
        "P+0Y1M2DT16H19M40S",
        false,
    ),
    (
        "test_time_fall_type2_prev_type2_dt",
        "2010-11-07 00:10:20 EDT",
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT5H31M52S",
        false,
    ),
    (
        "test_time_fall_type2_prev_type2_redodt",
        "2010-11-07 01:12:33 EDT",
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT6H34M5S",
        false,
    ),
    (
        "test_time_fall_type2_prev_type2_redost",
        "2010-11-07 01:14:44 EST",
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT7H36M16S",
        false,
    ),
    (
        "test_time_fall_type2_prev_type2_st",
        "2010-11-07 03:16:55 EST",
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M0DT9H38M27S",
        false,
    ),
    (
        "test_time_fall_type2_prev_type2_post",
        "2010-11-08 19:59:59 EST",
        "2010-11-06 18:38:28 EDT",
        "P+0Y0M2DT1H21M31S",
        false,
    ),
    (
        "test_time_fall_type2_dt_type2_prev",
        "2010-11-06 18:38:28 EDT",
        "2010-11-07 00:10:20 EDT",
        "P-0Y0M0DT5H31M52S",
        false,
    ),
    (
        "test_time_fall_type2_dt_type2_dt",
        "2010-11-07 00:15:35 EDT",
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT0H5M15S",
        false,
    ),
    (
        "test_time_fall_type2_dt_type2_redodt",
        "2010-11-07 01:12:33 EDT",
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT1H2M13S",
        false,
    ),
    (
        "test_time_fall_type2_dt_type2_redost",
        "2010-11-07 01:14:44 EST",
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT2H4M24S",
        false,
    ),
    (
        "test_time_fall_type2_dt_type2_st",
        "2010-11-07 03:16:55 EST",
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M0DT4H6M35S",
        false,
    ),
    (
        "test_time_fall_type2_dt_type2_post",
        "2010-11-08 19:59:59 EST",
        "2010-11-07 00:10:20 EDT",
        "P+0Y0M1DT20H49M39S",
        false,
    ),
    (
        "test_time_fall_type2_redodt_type2_prev",
        "2010-11-06 18:38:28 EDT",
        "2010-11-07 01:12:33 EDT",
        "P-0Y0M0DT6H34M5S",
        false,
    ),
    (
        "test_time_fall_type2_redodt_type2_dt",
        "2010-11-07 00:10:20 EDT",
        "2010-11-07 01:12:33 EDT",
        "P-0Y0M0DT1H2M13S",
        false,
    ),
    (
        "test_time_fall_type2_redodt_type2_redodt",
        "2010-11-07 01:15:35 EDT",
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M0DT0H3M2S",
        false,
    ),
    (
        "test_time_fall_type2_redodt_type2_redost",
        "2010-11-07 01:14:44 EST",
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M0DT1H2M11S",
        false,
    ),
    (
        "test_time_fall_type2_redodt_type2_st",
        "2010-11-07 03:16:55 EST",
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M0DT3H4M22S",
        false,
    ),
    (
        "test_time_fall_type2_redodt_type2_post",
        "2010-11-08 19:59:59 EST",
        "2010-11-07 01:12:33 EDT",
        "P+0Y0M1DT19H47M26S",
        false,
    ),
    (
        "test_time_fall_type2_redost_type2_prev",
        "2010-11-06 18:38:28 EDT",
        "2010-11-07 01:14:44 EST",
        "P-0Y0M0DT7H36M16S",
        false,
    ),
    (
        "test_time_fall_type2_redost_type2_dt",
        "2010-11-07 00:10:20 EDT",
        "2010-11-07 01:14:44 EST",
        "P-0Y0M0DT2H4M24S",
        false,
    ),
    (
        "test_time_fall_type2_redost_type2_redodt",
        "2010-11-07 01:12:33 EDT",
        "2010-11-07 01:14:44 EST",
        "P-0Y0M0DT1H2M11S",
        false,
    ),
    (
        "test_time_fall_type2_redost_type2_redost",
        "2010-11-07 01:16:54 EST",
        "2010-11-07 01:14:44 EST",
        "P+0Y0M0DT0H2M10S",
        false,
    ),
    (
        "test_time_fall_type2_redost_type2_st",
        "2010-11-07 03:16:55 EST",
        "2010-11-07 01:14:44 EST",
        "P+0Y0M0DT2H2M11S",
        false,
    ),
    (
        "test_time_fall_type2_redost_type2_post",
        "2010-11-08 19:59:59 EST",
        "2010-11-07 01:14:44 EST",
        "P+0Y0M1DT18H45M15S",
        false,
    ),
    (
        "test_time_fall_type2_st_type2_prev",
        "2010-11-06 18:38:28 EDT",
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT9H38M27S",
        false,
    ),
    (
        "test_time_fall_type2_st_type2_dt",
        "2010-11-07 00:10:20 EDT",
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT4H6M35S",
        false,
    ),
    (
        "test_time_fall_type2_st_type2_redodt",
        "2010-11-07 01:12:33 EDT",
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT3H4M22S",
        false,
    ),
    (
        "test_time_fall_type2_st_type2_redost",
        "2010-11-07 01:14:44 EST",
        "2010-11-07 03:16:55 EST",
        "P-0Y0M0DT2H2M11S",
        false,
    ),
    (
        "test_time_fall_type2_st_type2_st",
        "2010-11-07 05:19:56 EST",
        "2010-11-07 03:16:55 EST",
        "P+0Y0M0DT2H3M1S",
        false,
    ),
    (
        "test_time_fall_type2_st_type2_post",
        "2010-11-08 19:59:59 EST",
        "2010-11-07 03:16:55 EST",
        "P+0Y0M1DT16H43M4S",
        false,
    ),
    (
        "test_time_fall_type2_post_type2_prev",
        "2010-11-06 18:38:28 EDT",
        "2010-11-08 19:59:59 EST",
        "P-0Y0M2DT1H21M31S",
        false,
    ),
    (
        "test_time_fall_type2_post_type2_dt",
        "2010-11-07 00:10:20 EDT",
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT20H49M39S",
        false,
    ),
    (
        "test_time_fall_type2_post_type2_redodt",
        "2010-11-07 01:12:33 EDT",
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT19H47M26S",
        false,
    ),
    (
        "test_time_fall_type2_post_type2_redost",
        "2010-11-07 01:14:44 EST",
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT18H45M15S",
        false,
    ),
    (
        "test_time_fall_type2_post_type2_st",
        "2010-11-07 03:16:55 EST",
        "2010-11-08 19:59:59 EST",
        "P-0Y0M1DT16H43M4S",
        false,
    ),
    (
        "test_time_fall_type2_post_type2_post",
        "2010-11-08 19:59:59 EST",
        "2010-11-08 18:57:55 EST",
        "P+0Y0M0DT1H2M4S",
        false,
    ),
    (
        "test_time_fall_type2_dtsec_type2_stsec",
        "2010-11-07 01:00:00 EST",
        "2010-11-07 01:59:59 EDT",
        "P+0Y0M0DT0H0M1S",
        false,
    ),
    (
        "test_time_fall_type2_stsec_type2_dtsec",
        "2010-11-07 01:59:59 EDT",
        "2010-11-07 01:00:00 EST",
        "P-0Y0M0DT0H0M1S",
        false,
    ),
];
const DATA_SPRING_TYPE2_TYPE2: &[Case] = &[
    (
        "test_time_spring_type2_prev_type2_prev",
        "2010-03-13 18:38:28 EST",
        "2010-02-11 02:18:48 EST",
        "P+0Y1M2DT16H19M40S",
        false,
    ),
    (
        "test_time_spring_type2_prev_type2_st",
        "2010-03-14 00:10:20 EST",
        "2010-03-13 18:38:28 EST",
        "P+0Y0M0DT5H31M52S",
        false,
    ),
    (
        "test_time_spring_type2_prev_type2_dt",
        "2010-03-14 03:16:55 EDT",
        "2010-03-13 18:38:28 EST",
        "P+0Y0M0DT7H38M27S",
        false,
    ),
    (
        "test_time_spring_type2_prev_type2_post",
        "2010-03-15 19:59:59 EDT",
        "2010-03-13 18:38:28 EST",
        "P+0Y0M2DT1H21M31S",
        false,
    ),
    (
        "test_time_spring_type2_st_type2_prev",
        "2010-03-13 18:38:28 EST",
        "2010-03-14 00:10:20 EST",
        "P-0Y0M0DT5H31M52S",
        false,
    ),
    (
        "test_time_spring_type2_st_type2_st",
        "2010-03-14 00:15:35 EST",
        "2010-03-14 00:10:20 EST",
        "P+0Y0M0DT0H5M15S",
        false,
    ),
    (
        "test_time_spring_type2_st_type2_dt",
        "2010-03-14 03:16:55 EDT",
        "2010-03-14 00:10:20 EST",
        "P+0Y0M0DT2H6M35S",
        false,
    ),
    (
        "test_time_spring_type2_st_type2_post",
        "2010-03-15 19:59:59 EDT",
        "2010-03-14 00:10:20 EST",
        "P+0Y0M1DT18H49M39S",
        false,
    ),
    (
        "test_time_spring_type2_dt_type2_prev",
        "2010-03-13 18:38:28 EST",
        "2010-03-14 03:16:55 EDT",
        "P-0Y0M0DT7H38M27S",
        false,
    ),
    (
        "test_time_spring_type2_dt_type2_st",
        "2010-03-14 00:10:20 EST",
        "2010-03-14 03:16:55 EDT",
        "P-0Y0M0DT2H6M35S",
        false,
    ),
    (
        "test_time_spring_type2_dt_type2_dt",
        "2010-03-14 05:19:56 EDT",
        "2010-03-14 03:16:55 EDT",
        "P+0Y0M0DT2H3M1S",
        false,
    ),
    (
        "test_time_spring_type2_dt_type2_post",
        "2010-03-15 19:59:59 EDT",
        "2010-03-14 03:16:55 EDT",
        "P+0Y0M1DT16H43M4S",
        false,
    ),
    (
        "test_time_spring_type2_post_type2_prev",
        "2010-03-13 18:38:28 EST",
        "2010-03-15 19:59:59 EDT",
        "P-0Y0M2DT1H21M31S",
        false,
    ),
    (
        "test_time_spring_type2_post_type2_st",
        "2010-03-14 00:10:20 EST",
        "2010-03-15 19:59:59 EDT",
        "P-0Y0M1DT18H49M39S",
        false,
    ),
    (
        "test_time_spring_type2_post_type2_dt",
        "2010-03-14 03:16:55 EDT",
        "2010-03-15 19:59:59 EDT",
        "P-0Y0M1DT16H43M4S",
        false,
    ),
    (
        "test_time_spring_type2_post_type2_post",
        "2010-03-15 19:59:59 EDT",
        "2010-03-15 18:57:55 EDT",
        "P+0Y0M0DT1H2M4S",
        false,
    ),
    (
        "test_time_spring_type2_stsec_type2_dtsec",
        "2010-03-14 03:00:00 EDT",
        "2010-03-14 01:59:59 EST",
        "P+0Y0M0DT0H0M1S",
        false,
    ),
    (
        "test_time_spring_type2_dtsec_type2_stsec",
        "2010-03-14 01:59:59 EST",
        "2010-03-14 03:00:00 EDT",
        "P-0Y0M0DT0H0M1S",
        false,
    ),
];

// ---------------------------------------------------------------------------------
// Tests

/// Sanity checks for the EXPECTF matcher used below (not a phpt port).
#[test]
fn expectf_matcher_self_test() {
    assert!(expectf_match(b"int(12) abc", b"int(%d) %s"));
    assert!(expectf_match(b"x\ny", b"%a"));
    assert!(expectf_match(b"-5 +7 9", b"%i %i %i"));
    assert!(expectf_match(b"100%", b"%d%%"));
    assert!(!expectf_match(b"int() abc", b"int(%d) %s"));
    assert!(!expectf_match(b"x\ny", b"%s"));
    assert!(!expectf_match(b"abc", b"abd"));
    assert!(!expectf_match(b"abc", b"ab"));
    assert!(!expectf_match(b"ab", b"abc"));
}

/// ext/date/tests/006.phpt
#[test]
fn t_006() {
    let checkdate =
        |m: i64, d: i64, y: i64| (1..=32767).contains(&y) && php_timelib::valid_date(y, m, d);
    let mut out = String::new();
    for (m, d, y) in [
        (1, 1, 1),
        (2, 29, 2006),
        (31, 6, 2006),
        (1, 1, 32768),
        (1, 1, 32767),
        (-1, 1, 2006),
        (1, -1, 2006),
        (1, 1, -1),
    ] {
        out += &dump_bool(checkdate(m, d, y));
    }
    out += "Done\n";
    assert_text(
        &out,
        r#"bool(true)
bool(false)
bool(false)
bool(false)
bool(true)
bool(false)
bool(false)
bool(false)
Done"#,
    );
}

/// ext/date/tests/011.phpt
#[test]
fn t_011() {
    let f = |a: &str, o: i64, d: i32| match php_timelib::timezone_id_from_abbr(a, o, d) {
        Some(s) => dump_str(s),
        None => dump_bool(false),
    };
    let out = f("CET", -1, -1) + &f("AXST", -1, -1) + &f("", 3600, -1) + &f("", 3600, 0) + "Done\n";
    assert_text(
        &out,
        r#"string(13) "Europe/Berlin"
bool(false)
bool(false)
string(12) "Europe/Paris"
Done"#,
    );
}

/// ext/date/tests/DateInterval_construct_exceptions.phpt
#[test]
fn dateinterval_construct_exceptions() {
    let mut out = String::new();
    for spec in ["", "2007-05-11T15:30:00Z/"] {
        match php::date_interval_create(spec) {
            Ok(iv) => out += &interval_var_dump(&iv, 1),
            Err(e) => {
                let _ = writeln!(out, "DateMalformedIntervalStringException: {e}");
            }
        }
    }
    assert_expectf(
        &out,
        r#"DateMalformedIntervalStringException: Unknown or bad format ()
DateMalformedIntervalStringException: Failed to parse interval (2007-05-11T15:30:00Z/)"#,
    );
}

/// ext/date/tests/DateInterval_serialize-002.phpt (partial: the original diff and the
/// add/sub with the unserialised interval, which unserialize() restores as a civil
/// interval with the same y/m/d/h/i/s/f/invert; the serialised strings and the manual
/// `__unserialize()` call are not ported)
#[test]
fn dateinterval_serialize_002() {
    let z = "Europe/London";
    let d1 = DT::new("1978-12-22 09:15 CET", z);
    let d2 = DT::new("2022-04-15 10:27:27 BST", z);
    let d = d1.diff(&d2);
    let out = String::from("Original object:\n") + &interval_var_dump(&d, 3);
    assert_text(
        &out,
        r#"Original object:
object(DateInterval)#3 (10) {
  ["y"]=>
  int(43)
  ["m"]=>
  int(3)
  ["d"]=>
  int(24)
  ["h"]=>
  int(1)
  ["i"]=>
  int(12)
  ["s"]=>
  int(27)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  int(15820)
  ["from_string"]=>
  bool(false)
}"#,
    );

    let now = DT::new("2022-04-15 10:27:27 BST", z);
    let mut a = now.clone();
    a.add_interval(&d);
    let mut s = now.clone();
    s.sub_interval(&d);
    let out = String::from("Used serialised interval:\n")
        + &a.var_dump("DateTimeImmutable", 6)
        + &s.var_dump("DateTimeImmutable", 6);
    assert_text(
        &out,
        r#"Used serialised interval:
object(DateTimeImmutable)#6 (3) {
  ["date"]=>
  string(26) "2065-08-08 11:39:54.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "BST"
}
object(DateTimeImmutable)#6 (3) {
  ["date"]=>
  string(26) "1978-12-22 09:15:00.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "BST"
}"#,
    );
}

/// ext/date/tests/DatePeriod_properties1.phpt (partial: only the interval line; the
/// other lines are DatePeriod property plumbing on "now")
#[test]
fn dateperiod_properties1() {
    let iv = php::date_interval_create("P1D").unwrap();
    assert_eq!(
        format!("interval: {}", dump_str(&interval_format(&iv, "%R%d"))),
        "interval: string(2) \"+1\"\n"
    );
}

/// ext/date/tests/DatePeriod_set_state.phpt (var_export()/eval()/__set_state() are
/// emulated: the start date goes through `php_date_initialize_from_hash()`, the
/// interval and the other properties are copied as-is)
#[test]
fn dateperiod_set_state() {
    let start = DT::new_in("2017-10-06 23:30", "UTC", "UTC");
    let interval = php::date_interval_create("PT1H30M").unwrap();
    let start_new = date_roundtrip(&start).unwrap();
    let out = String::from("object(DatePeriod)#3 (7) {\n")
        + &period_props_dump("DateTime", &start_new, None, &interval, 24 + 1, true, false)
        + "}\n";
    assert_expectf(
        &out,
        r#"object(DatePeriod)#%d (%d) {
  ["start"]=>
  object(DateTime)#%d (%d) {
    ["date"]=>
    string(26) "2017-10-06 23:30:00.000000"
    ["timezone_type"]=>
    int(3)
    ["timezone"]=>
    string(3) "UTC"
  }
  ["current"]=>
  NULL
  ["end"]=>
  NULL
  ["interval"]=>
  object(DateInterval)#%d (%d) {
    ["y"]=>
    int(0)
    ["m"]=>
    int(0)
    ["d"]=>
    int(0)
    ["h"]=>
    int(1)
    ["i"]=>
    int(30)
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
  int(25)
  ["include_start_date"]=>
  bool(true)
  ["include_end_date"]=>
  bool(false)
}"#,
    );
}

/// ext/date/tests/DateTimeZone_clone_basic3.phpt (partial: the freshly created object;
/// dynamic properties and clone are engine behaviour)
#[test]
fn datetimezone_clone_basic3() {
    let d1 = php::timezone_open("Europe/London").unwrap();
    assert_expectf(
        &tz_var_dump(&d1, 1),
        r#"object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}"#,
    );
}

/// ext/date/tests/DateTimeZone_extends_basic1.phpt (partial: getName() of the zone)
#[test]
fn datetimezone_extends_basic1() {
    let d = php::timezone_open("America/Los_Angeles").unwrap();
    assert_eq!(d.name(), "America/Los_Angeles");
}

/// ext/date/tests/DateTimeZone_serialize_type_2.phpt (partial: the serialised string is
/// not ported; unserialize() re-runs the DateTimeZone constructor on the stored name)
#[test]
fn datetimezone_serialize_type_2() {
    let tz1 = php::timezone_open("EST").unwrap();
    let tz2 = php::timezone_open(&tz1.name()).unwrap();
    let out = tz_var_dump(&tz1, 1) + &tz_var_dump(&tz2, 2) + &dump_str(&tz2.name());
    assert_expectf(
        &out,
        r#"object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "EST"
}
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "EST"
}
string(3) "EST""#,
    );
}

/// ext/date/tests/DateTime_add-fall-type2-type2.phpt
#[test]
fn datetime_add_fall_type2_type2() {
    let out = examine_diff(DATA_FALL_TYPE2_TYPE2, Show::Add, "America/New_York");
    assert_text(
        &out,
        r#"test_time_fall_type2_prev_type2_prev: ADD: 2010-10-04 02:18:48 EDT + P+0Y1M2DT16H19M40S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type2_dt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_prev_type2_redodt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_prev_type2_redost: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT7H36M16S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_prev_type2_st: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT9H38M27S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_prev_type2_post: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EDT**
test_time_fall_type2_dt_type2_prev: ADD: 2010-11-07 00:10:20 EDT + P-0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_dt_type2_dt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT0H5M15S = **2010-11-07 00:15:35 EDT**
test_time_fall_type2_dt_type2_redodt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_dt_type2_redost: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT2H4M24S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_dt_type2_st: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT4H6M35S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_dt_type2_post: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EDT**
test_time_fall_type2_redodt_type2_prev: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_redodt_type2_dt: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_redodt_type2_redodt: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT0H3M2S = **2010-11-07 01:15:35 EDT**
test_time_fall_type2_redodt_type2_redost: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT1H2M11S = **2010-11-07 02:14:44 EDT**
test_time_fall_type2_redodt_type2_st: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT3H4M22S = **2010-11-07 04:16:55 EDT**
test_time_fall_type2_redodt_type2_post: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EDT**
test_time_fall_type2_redost_type2_prev: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT7H36M16S = **2010-11-06 17:38:28 EST**
test_time_fall_type2_redost_type2_dt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT2H4M24S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_redost_type2_redodt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT1H2M11S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_redost_type2_redost: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT0H2M10S = **2010-11-07 01:16:54 EST**
test_time_fall_type2_redost_type2_st: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_redost_type2_post: ADD: 2010-11-07 01:14:44 EST + P+0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_st_type2_prev: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT9H38M27S = **2010-11-06 17:38:28 EST**
test_time_fall_type2_st_type2_dt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT4H6M35S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_st_type2_redodt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT3H4M22S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_st_type2_redost: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_st_type2_st: ADD: 2010-11-07 03:16:55 EST + P+0Y0M0DT2H3M1S = **2010-11-07 05:19:56 EST**
test_time_fall_type2_st_type2_post: ADD: 2010-11-07 03:16:55 EST + P+0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_post_type2_prev: ADD: 2010-11-08 19:59:59 EST + P-0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EST**
test_time_fall_type2_post_type2_dt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT20H49M39S = **2010-11-06 23:10:20 EST**
test_time_fall_type2_post_type2_redodt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT19H47M26S = **2010-11-07 00:12:33 EST**
test_time_fall_type2_post_type2_redost: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_post_type2_st: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_post_type2_post: ADD: 2010-11-08 18:57:55 EST + P+0Y0M0DT1H2M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_dtsec_type2_stsec: ADD: 2010-11-07 01:59:59 EDT + P+0Y0M0DT0H0M1S = **2010-11-07 02:00:00 EDT**
test_time_fall_type2_stsec_type2_dtsec: ADD: 2010-11-07 01:00:00 EST + P-0Y0M0DT0H0M1S = **2010-11-07 00:59:59 EST**"#,
    );
}

/// ext/date/tests/DateTime_add-spring-type2-type2.phpt
#[test]
fn datetime_add_spring_type2_type2() {
    let out = examine_diff(DATA_SPRING_TYPE2_TYPE2, Show::Add, "America/New_York");
    assert_text(
        &out,
        r#"test_time_spring_type2_prev_type2_prev: ADD: 2010-02-11 02:18:48 EST + P+0Y1M2DT16H19M40S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_prev_type2_st: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_prev_type2_dt: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT7H38M27S = **2010-03-14 02:16:55 EST**
test_time_spring_type2_prev_type2_post: ADD: 2010-03-13 18:38:28 EST + P+0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EST**
test_time_spring_type2_st_type2_prev: ADD: 2010-03-14 00:10:20 EST + P-0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_st_type2_st: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT0H5M15S = **2010-03-14 00:15:35 EST**
test_time_spring_type2_st_type2_dt: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT2H6M35S = **2010-03-14 02:16:55 EST**
test_time_spring_type2_st_type2_post: ADD: 2010-03-14 00:10:20 EST + P+0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EST**
test_time_spring_type2_dt_type2_prev: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT7H38M27S = **2010-03-13 19:38:28 EDT**
test_time_spring_type2_dt_type2_st: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT2H6M35S = **2010-03-14 01:10:20 EDT**
test_time_spring_type2_dt_type2_dt: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M0DT2H3M1S = **2010-03-14 05:19:56 EDT**
test_time_spring_type2_dt_type2_post: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type2_post_type2_prev: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EDT**
test_time_spring_type2_post_type2_st: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT18H49M39S = **2010-03-14 01:10:20 EDT**
test_time_spring_type2_post_type2_dt: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_post_type2_post: ADD: 2010-03-15 18:57:55 EDT + P+0Y0M0DT1H2M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type2_stsec_type2_dtsec: ADD: 2010-03-14 01:59:59 EST + P+0Y0M0DT0H0M1S = **2010-03-14 02:00:00 EST**
test_time_spring_type2_dtsec_type2_stsec: ADD: 2010-03-14 03:00:00 EDT + P-0Y0M0DT0H0M1S = **2010-03-14 02:59:59 EDT**"#,
    );
}

/// ext/date/tests/DateTime_clone_basic3.phpt (partial: the freshly created object;
/// dynamic properties and clone are engine behaviour)
#[test]
fn datetime_clone_basic3() {
    let d1 = DT::new("2009-02-03 12:34:41 GMT", "Europe/London");
    assert_expectf(
        &d1.var_dump("DateTime", 1),
        r#"object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2009-02-03 12:34:41.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "GMT"
}"#,
    );
}

/// ext/date/tests/DateTime_construct_basic1.phpt ("now" is the fixed `common::NOW`; the
/// EXPECTF does not check those dates)
#[test]
fn datetime_construct_basic1() {
    let z = "Europe/London";
    let mut out = String::from("*** Testing new DateTime() : basic functionality ***\n");
    for (i, s) in ["", "GMT", "2005-07-14 22:30:41", "2005-07-14 22:30:41 GMT"]
        .into_iter()
        .enumerate()
    {
        out += &DT::new(s, z).var_dump("DateTime", i as u32 + 1);
    }
    assert_expectf(
        &out,
        r#"*** Testing new DateTime() : basic functionality ***
object(DateTime)#%d (3) {
  ["date"]=>
  string(%d) "%s"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(%d) "%s"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "GMT"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2005-07-14 22:30:41.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2005-07-14 22:30:41.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "GMT"
}"#,
    );
}

/// ext/date/tests/DateTime_days-absolute.phpt
#[test]
fn datetime_days_absolute() {
    let out = examine_diff(DATA_ABSOLUTE, Show::Days, "America/New_York");
    assert_text(
        &out,
        r#"test_absolute_7: DAYS: **7**
test_absolute_negative_7: DAYS: **7**"#,
    );
}

/// ext/date/tests/DateTime_days-february.phpt
#[test]
fn datetime_days_february() {
    let out = examine_diff(DATA_FEBRUARY, Show::Days, "America/New_York");
    assert_text(
        &out,
        r#"test_bug_49081__1: DAYS: **30**
test_bug_49081__2: DAYS: **31**
test_bug_49081__3: DAYS: **1**
test_bug_49081__4: DAYS: **29**
test_bug_49081__5: DAYS: **30**
test_bug_49081__6: DAYS: **31**
test_bug_49081__7: DAYS: **32**
test_bug_49081__8: DAYS: **28**
test_bug_49081__9: DAYS: **29**
test_bug_49081__10: DAYS: **30**
test_bug_49081__11: DAYS: **31**
test_bug_49081__12: DAYS: **1**
test_bug_49081__13: DAYS: **27**
test_bug_49081__14: DAYS: **28**
test_bug_49081__15: DAYS: **29**
test_bug_49081__16: DAYS: **30**
test_bug_49081__17: DAYS: **31**
test_bug_49081__18: DAYS: **32**
test_bug_49081__19: DAYS: **59**
test_bug_49081__20: DAYS: **29**
test_bug_49081__21: DAYS: **55**
test_bug_49081__22: DAYS: **56**
test_bug_49081__23: DAYS: **57**
test_bug_49081__24: DAYS: **58**
test_bug_49081__25: DAYS: **59**
test_bug_49081__26: DAYS: **60**
test_bug_49081__27: DAYS: **30**
test_bug_49081__28: DAYS: **27**
test_bug_49081__29: DAYS: **28**
test_bug_49081__30: DAYS: **29**
test_bug_49081__31: DAYS: **28**
test_bug_49081__32: DAYS: **29**
test_bug_49081_negative__1: DAYS: **30**
test_bug_49081_negative__2: DAYS: **31**
test_bug_49081_negative__3: DAYS: **1**
test_bug_49081_negative__4: DAYS: **29**
test_bug_49081_negative__5: DAYS: **30**
test_bug_49081_negative__6: DAYS: **31**
test_bug_49081_negative__7: DAYS: **32**
test_bug_49081_negative__8: DAYS: **28**
test_bug_49081_negative__9: DAYS: **29**
test_bug_49081_negative__10: DAYS: **30**
test_bug_49081_negative__11: DAYS: **31**
test_bug_49081_negative__12: DAYS: **1**
test_bug_49081_negative__13: DAYS: **27**
test_bug_49081_negative__14: DAYS: **28**
test_bug_49081_negative__15: DAYS: **29**
test_bug_49081_negative__16: DAYS: **30**
test_bug_49081_negative__17: DAYS: **31**
test_bug_49081_negative__18: DAYS: **32**
test_bug_49081_negative__19: DAYS: **59**
test_bug_49081_negative__20: DAYS: **29**
test_bug_49081_negative__21: DAYS: **55**
test_bug_49081_negative__22: DAYS: **56**
test_bug_49081_negative__23: DAYS: **57**
test_bug_49081_negative__24: DAYS: **58**
test_bug_49081_negative__25: DAYS: **59**
test_bug_49081_negative__26: DAYS: **60**
test_bug_49081_negative__27: DAYS: **30**
test_bug_49081_negative__28: DAYS: **27**
test_bug_49081_negative__29: DAYS: **28**
test_bug_49081_negative__30: DAYS: **29**
test_bug_49081_negative__31: DAYS: **28**
test_bug_49081_negative__32: DAYS: **29**"#,
    );
}

/// ext/date/tests/DateTime_diff-absolute.phpt
#[test]
fn datetime_diff_absolute() {
    let out = examine_diff(DATA_ABSOLUTE, Show::Diff, "America/New_York");
    assert_text(
        &out,
        r#"test_absolute_7: DIFF: 2009-01-14 00:00:00 EST - 2009-01-07 00:00:00 EST = **P+0Y0M7DT0H0M0S**
test_absolute_negative_7: DIFF: 2009-01-07 00:00:00 EST - 2009-01-14 00:00:00 EST = **P+0Y0M7DT0H0M0S**"#,
    );
}

/// ext/date/tests/DateTime_diff-february.phpt
#[test]
fn datetime_diff_february() {
    let out = examine_diff(DATA_FEBRUARY, Show::Diff, "America/New_York");
    assert_text(
        &out,
        r#"test_bug_49081__1: DIFF: 2010-03-31 00:00:00 EDT - 2010-03-01 00:00:00 EST = **P+0Y0M30DT0H0M0S**
test_bug_49081__2: DIFF: 2010-04-01 00:00:00 EDT - 2010-03-01 00:00:00 EST = **P+0Y1M0DT0H0M0S**
test_bug_49081__3: DIFF: 2010-04-01 00:00:00 EDT - 2010-03-31 00:00:00 EDT = **P+0Y0M1DT0H0M0S**
test_bug_49081__4: DIFF: 2010-04-29 00:00:00 EDT - 2010-03-31 00:00:00 EDT = **P+0Y0M29DT0H0M0S**
test_bug_49081__5: DIFF: 2010-04-30 00:00:00 EDT - 2010-03-31 00:00:00 EDT = **P+0Y0M30DT0H0M0S**
test_bug_49081__6: DIFF: 2010-04-30 00:00:00 EDT - 2010-03-30 00:00:00 EDT = **P+0Y1M0DT0H0M0S**
test_bug_49081__7: DIFF: 2010-04-30 00:00:00 EDT - 2010-03-29 00:00:00 EDT = **P+0Y1M1DT0H0M0S**
test_bug_49081__8: DIFF: 2010-01-29 00:00:00 EST - 2010-01-01 00:00:00 EST = **P+0Y0M28DT0H0M0S**
test_bug_49081__9: DIFF: 2010-01-30 00:00:00 EST - 2010-01-01 00:00:00 EST = **P+0Y0M29DT0H0M0S**
test_bug_49081__10: DIFF: 2010-01-31 00:00:00 EST - 2010-01-01 00:00:00 EST = **P+0Y0M30DT0H0M0S**
test_bug_49081__11: DIFF: 2010-02-01 00:00:00 EST - 2010-01-01 00:00:00 EST = **P+0Y1M0DT0H0M0S**
test_bug_49081__12: DIFF: 2010-02-01 00:00:00 EST - 2010-01-31 00:00:00 EST = **P+0Y0M1DT0H0M0S**
test_bug_49081__13: DIFF: 2010-02-27 00:00:00 EST - 2010-01-31 00:00:00 EST = **P+0Y0M27DT0H0M0S**
test_bug_49081__14: DIFF: 2010-02-28 00:00:00 EST - 2010-01-31 00:00:00 EST = **P+0Y0M28DT0H0M0S**
test_bug_49081__15: DIFF: 2010-02-28 00:00:00 EST - 2010-01-30 00:00:00 EST = **P+0Y0M29DT0H0M0S**
test_bug_49081__16: DIFF: 2010-02-28 00:00:00 EST - 2010-01-29 00:00:00 EST = **P+0Y0M30DT0H0M0S**
test_bug_49081__17: DIFF: 2010-02-28 00:00:00 EST - 2010-01-28 00:00:00 EST = **P+0Y1M0DT0H0M0S**
test_bug_49081__18: DIFF: 2010-02-28 00:00:00 EST - 2010-01-27 00:00:00 EST = **P+0Y1M1DT0H0M0S**
test_bug_49081__19: DIFF: 2010-03-01 00:00:00 EST - 2010-01-01 00:00:00 EST = **P+0Y2M0DT0H0M0S**
test_bug_49081__20: DIFF: 2010-03-01 00:00:00 EST - 2010-01-31 00:00:00 EST = **P+0Y0M29DT0H0M0S**
test_bug_49081__21: DIFF: 2010-03-27 00:00:00 EDT - 2010-01-31 00:00:00 EST = **P+0Y1M24DT0H0M0S**
test_bug_49081__22: DIFF: 2010-03-28 00:00:00 EDT - 2010-01-31 00:00:00 EST = **P+0Y1M25DT0H0M0S**
test_bug_49081__23: DIFF: 2010-03-29 00:00:00 EDT - 2010-01-31 00:00:00 EST = **P+0Y1M26DT0H0M0S**
test_bug_49081__24: DIFF: 2010-03-30 00:00:00 EDT - 2010-01-31 00:00:00 EST = **P+0Y1M27DT0H0M0S**
test_bug_49081__25: DIFF: 2010-03-31 00:00:00 EDT - 2010-01-31 00:00:00 EST = **P+0Y2M0DT0H0M0S**
test_bug_49081__26: DIFF: 2010-03-31 00:00:00 EDT - 2010-01-30 00:00:00 EST = **P+0Y2M1DT0H0M0S**
test_bug_49081__27: DIFF: 2009-01-31 00:00:00 EST - 2009-01-01 00:00:00 EST = **P+0Y0M30DT0H0M0S**
test_bug_49081__28: DIFF: 2010-03-27 00:00:00 EDT - 2010-02-28 00:00:00 EST = **P+0Y0M27DT0H0M0S**
test_bug_49081__29: DIFF: 2010-03-28 00:00:00 EDT - 2010-02-28 00:00:00 EST = **P+0Y1M0DT0H0M0S**
test_bug_49081__30: DIFF: 2010-03-29 00:00:00 EDT - 2010-02-28 00:00:00 EST = **P+0Y1M1DT0H0M0S**
test_bug_49081__31: DIFF: 2010-03-27 00:00:00 EDT - 2010-02-27 00:00:00 EST = **P+0Y1M0DT0H0M0S**
test_bug_49081__32: DIFF: 2010-03-27 00:00:00 EDT - 2010-02-26 00:00:00 EST = **P+0Y1M1DT0H0M0S**
test_bug_49081_negative__1: DIFF: 2010-03-01 00:00:00 EST - 2010-03-31 00:00:00 EDT = **P-0Y0M30DT0H0M0S**
test_bug_49081_negative__2: DIFF: 2010-03-01 00:00:00 EST - 2010-04-01 00:00:00 EDT = **P-0Y1M0DT0H0M0S**
test_bug_49081_negative__3: DIFF: 2010-03-31 00:00:00 EDT - 2010-04-01 00:00:00 EDT = **P-0Y0M1DT0H0M0S**
test_bug_49081_negative__4: DIFF: 2010-03-31 00:00:00 EDT - 2010-04-29 00:00:00 EDT = **P-0Y0M29DT0H0M0S**
test_bug_49081_negative__5: DIFF: 2010-03-31 00:00:00 EDT - 2010-04-30 00:00:00 EDT = **P-0Y0M30DT0H0M0S**
test_bug_49081_negative__6: DIFF: 2010-03-30 00:00:00 EDT - 2010-04-30 00:00:00 EDT = **P-0Y1M0DT0H0M0S**
test_bug_49081_negative__7: DIFF: 2010-03-29 00:00:00 EDT - 2010-04-30 00:00:00 EDT = **P-0Y1M1DT0H0M0S**
test_bug_49081_negative__8: DIFF: 2010-01-01 00:00:00 EST - 2010-01-29 00:00:00 EST = **P-0Y0M28DT0H0M0S**
test_bug_49081_negative__9: DIFF: 2010-01-01 00:00:00 EST - 2010-01-30 00:00:00 EST = **P-0Y0M29DT0H0M0S**
test_bug_49081_negative__10: DIFF: 2010-01-01 00:00:00 EST - 2010-01-31 00:00:00 EST = **P-0Y0M30DT0H0M0S**
test_bug_49081_negative__11: DIFF: 2010-01-01 00:00:00 EST - 2010-02-01 00:00:00 EST = **P-0Y1M0DT0H0M0S**
test_bug_49081_negative__12: DIFF: 2010-01-31 00:00:00 EST - 2010-02-01 00:00:00 EST = **P-0Y0M1DT0H0M0S**
test_bug_49081_negative__13: DIFF: 2010-01-31 00:00:00 EST - 2010-02-27 00:00:00 EST = **P-0Y0M27DT0H0M0S**
test_bug_49081_negative__14: DIFF: 2010-01-31 00:00:00 EST - 2010-02-28 00:00:00 EST = **P-0Y0M28DT0H0M0S**
test_bug_49081_negative__15: DIFF: 2010-01-30 00:00:00 EST - 2010-02-28 00:00:00 EST = **P-0Y0M29DT0H0M0S**
test_bug_49081_negative__16: DIFF: 2010-01-29 00:00:00 EST - 2010-02-28 00:00:00 EST = **P-0Y0M30DT0H0M0S**
test_bug_49081_negative__17: DIFF: 2010-01-28 00:00:00 EST - 2010-02-28 00:00:00 EST = **P-0Y1M0DT0H0M0S**
test_bug_49081_negative__18: DIFF: 2010-01-27 00:00:00 EST - 2010-02-28 00:00:00 EST = **P-0Y1M1DT0H0M0S**
test_bug_49081_negative__19: DIFF: 2010-01-01 00:00:00 EST - 2010-03-01 00:00:00 EST = **P-0Y2M0DT0H0M0S**
test_bug_49081_negative__20: DIFF: 2010-01-31 00:00:00 EST - 2010-03-01 00:00:00 EST = **P-0Y1M1DT0H0M0S**
test_bug_49081_negative__21: DIFF: 2010-01-31 00:00:00 EST - 2010-03-27 00:00:00 EDT = **P-0Y1M27DT0H0M0S**
test_bug_49081_negative__22: DIFF: 2010-01-31 00:00:00 EST - 2010-03-28 00:00:00 EDT = **P-0Y1M28DT0H0M0S**
test_bug_49081_negative__23: DIFF: 2010-01-31 00:00:00 EST - 2010-03-29 00:00:00 EDT = **P-0Y1M29DT0H0M0S**
test_bug_49081_negative__24: DIFF: 2010-01-31 00:00:00 EST - 2010-03-30 00:00:00 EDT = **P-0Y1M30DT0H0M0S**
test_bug_49081_negative__25: DIFF: 2010-01-31 00:00:00 EST - 2010-03-31 00:00:00 EDT = **P-0Y2M0DT0H0M0S**
test_bug_49081_negative__26: DIFF: 2010-01-30 00:00:00 EST - 2010-03-31 00:00:00 EDT = **P-0Y2M1DT0H0M0S**
test_bug_49081_negative__27: DIFF: 2009-01-01 00:00:00 EST - 2009-01-31 00:00:00 EST = **P-0Y0M30DT0H0M0S**
test_bug_49081_negative__28: DIFF: 2010-02-28 00:00:00 EST - 2010-03-27 00:00:00 EDT = **P-0Y0M27DT0H0M0S**
test_bug_49081_negative__29: DIFF: 2010-02-28 00:00:00 EST - 2010-03-28 00:00:00 EDT = **P-0Y1M0DT0H0M0S**
test_bug_49081_negative__30: DIFF: 2010-02-28 00:00:00 EST - 2010-03-29 00:00:00 EDT = **P-0Y1M1DT0H0M0S**
test_bug_49081_negative__31: DIFF: 2010-02-27 00:00:00 EST - 2010-03-27 00:00:00 EDT = **P-0Y1M0DT0H0M0S**
test_bug_49081_negative__32: DIFF: 2010-02-26 00:00:00 EST - 2010-03-27 00:00:00 EDT = **P-0Y1M1DT0H0M0S**"#,
    );
}

/// ext/date/tests/DateTime_extends_basic1.phpt
#[test]
fn datetime_extends_basic1() {
    let f = "F j, Y, g:i:s a";
    let mut d = DT::new("1967-05-01 22:30:41", "Europe/London");
    let mut out = String::from(
        "*** Testing basic DateTime inheritance() ***\n\n-- Create an instance of DateTimeExt --\n\n-- Invoke __toString --\n",
    );
    out += &d.format(f);
    out += "\n\n -- modify date and time --\n";
    d.set_date(1963, 7, 2);
    d.set_time(10, 45, 30, 0);
    out += "\n-- Invoke __toString again --\n";
    out += &d.format(f);
    out += "\n";
    assert_text(
        &out,
        r#"*** Testing basic DateTime inheritance() ***

-- Create an instance of DateTimeExt --

-- Invoke __toString --
May 1, 1967, 10:30:41 pm

 -- modify date and time --

-- Invoke __toString again --
July 2, 1963, 10:45:30 am"#,
    );
}

/// ext/date/tests/DateTime_getOffset_basic1.phpt
#[test]
fn datetime_getoffset_basic1() {
    let z = "Europe/London";
    let winter = DT::new("2008-12-25 14:25:41", z);
    let summer = DT::new("2008-07-02 14:25:41", z);
    let out = format!(
        "*** Testing DateTime::getOffset() : basic functionality ***\nWinter offset: {} hours\nSummer offset: {} hours\n",
        winter.offset() / 3600,
        summer.offset() / 3600
    );
    assert_text(
        &out,
        r#"*** Testing DateTime::getOffset() : basic functionality ***
Winter offset: 0 hours
Summer offset: 1 hours"#,
    );
}

/// ext/date/tests/DateTime_serialize.phpt (partial: the serialised string is not
/// ported; unserialize() is emulated with `php_date_initialize_from_hash()`)
#[test]
fn datetime_serialize() {
    let date1 = DT::new("2005-07-14 22:30:41", "Europe/London");
    let date2 = date_roundtrip(&date1).unwrap();
    let out = date1.var_dump("DateTime", 1)
        + &date2.var_dump("DateTime", 2)
        + &dump_str(&date2.format("F j, Y, g:i a"));
    assert_expectf(
        &out,
        r#"object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2005-07-14 22:30:41.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2005-07-14 22:30:41.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
string(23) "July 14, 2005, 10:30 pm""#,
    );
}

/// ext/date/tests/DateTime_set_state_exception.phpt (partial: the property sets with
/// string/int types; the float-typed ones fail PHP's type checks before timelib runs)
#[test]
fn datetime_set_state_exception() {
    let mut out = String::new();
    for (date, tz_type, tz) in [
        ("2023-01-13 12:29:30", 1, "+02:30"),
        ("2023-01-13 12:29:30", 3, "Europe/Kyiv"),
        ("2023-01-13 12:29:30", 3, "Europe/Lviv"),
        ("2023-01-13 12:29:30", 4, "Europe/Kyiv"),
    ] {
        out += match date_from_hash(date, tz_type, tz, "UTC") {
            Some(_) => "OK\n",
            None => "Error: Invalid serialization data for DateTime object\n",
        };
    }
    assert_text(
        &out,
        "OK\nOK\nError: Invalid serialization data for DateTime object\nError: Invalid serialization data for DateTime object\n",
    );
}

/// ext/date/tests/DateTime_sub-february.phpt
#[test]
fn datetime_sub_february() {
    let out = examine_diff(DATA_FEBRUARY, Show::Sub, "America/New_York");
    assert_text(
        &out,
        r#"test_bug_49081__1: SUB: 2010-03-31 00:00:00 EDT - P+0Y0M30DT0H0M0S = **2010-03-01 00:00:00 EST**
test_bug_49081__2: SUB: 2010-04-01 00:00:00 EDT - P+0Y1M0DT0H0M0S = **2010-03-01 00:00:00 EST**
test_bug_49081__3: SUB: 2010-04-01 00:00:00 EDT - P+0Y0M1DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081__4: SUB: 2010-04-29 00:00:00 EDT - P+0Y0M29DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081__5: SUB: 2010-04-30 00:00:00 EDT - P+0Y0M30DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081__6: SUB: 2010-04-30 00:00:00 EDT - P+0Y1M0DT0H0M0S = **2010-03-30 00:00:00 EDT**
test_bug_49081__7: SUB: 2010-04-30 00:00:00 EDT - P+0Y1M1DT0H0M0S = **2010-03-29 00:00:00 EDT**
test_bug_49081__8: SUB: 2010-01-29 00:00:00 EST - P+0Y0M28DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081__9: SUB: 2010-01-30 00:00:00 EST - P+0Y0M29DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081__10: SUB: 2010-01-31 00:00:00 EST - P+0Y0M30DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081__11: SUB: 2010-02-01 00:00:00 EST - P+0Y1M0DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081__12: SUB: 2010-02-01 00:00:00 EST - P+0Y0M1DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081__13: SUB: 2010-02-27 00:00:00 EST - P+0Y0M27DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081__14: SUB: 2010-02-28 00:00:00 EST - P+0Y0M28DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081__15: SUB: 2010-02-28 00:00:00 EST - P+0Y0M29DT0H0M0S = **2010-01-30 00:00:00 EST**
test_bug_49081__16: SUB: 2010-02-28 00:00:00 EST - P+0Y0M30DT0H0M0S = **2010-01-29 00:00:00 EST**
test_bug_49081__17: SUB: 2010-02-28 00:00:00 EST - P+0Y1M0DT0H0M0S = **2010-01-28 00:00:00 EST**
test_bug_49081__18: SUB: 2010-02-28 00:00:00 EST - P+0Y1M1DT0H0M0S = **2010-01-27 00:00:00 EST**
test_bug_49081__19: SUB: 2010-03-01 00:00:00 EST - P+0Y2M0DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081__20: SUB: 2010-03-01 00:00:00 EST - P+0Y0M29DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081__21: SUB: 2010-03-27 00:00:00 EDT - P+0Y1M24DT0H0M0S = **2010-02-03 00:00:00 EST**
test_bug_49081__22: SUB: 2010-03-28 00:00:00 EDT - P+0Y1M25DT0H0M0S = **2010-02-03 00:00:00 EST**
test_bug_49081__23: SUB: 2010-03-29 00:00:00 EDT - P+0Y1M26DT0H0M0S = **2010-02-03 00:00:00 EST**
test_bug_49081__24: SUB: 2010-03-30 00:00:00 EDT - P+0Y1M27DT0H0M0S = **2010-02-03 00:00:00 EST**
test_bug_49081__25: SUB: 2010-03-31 00:00:00 EDT - P+0Y2M0DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081__26: SUB: 2010-03-31 00:00:00 EDT - P+0Y2M1DT0H0M0S = **2010-01-30 00:00:00 EST**
test_bug_49081__27: SUB: 2009-01-31 00:00:00 EST - P+0Y0M30DT0H0M0S = **2009-01-01 00:00:00 EST**
test_bug_49081__28: SUB: 2010-03-27 00:00:00 EDT - P+0Y0M27DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__29: SUB: 2010-03-28 00:00:00 EDT - P+0Y1M0DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__30: SUB: 2010-03-29 00:00:00 EDT - P+0Y1M1DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__31: SUB: 2010-03-27 00:00:00 EDT - P+0Y1M0DT0H0M0S = **2010-02-27 00:00:00 EST**
test_bug_49081__32: SUB: 2010-03-27 00:00:00 EDT - P+0Y1M1DT0H0M0S = **2010-02-26 00:00:00 EST**
test_bug_49081_negative__1: SUB: 2010-03-01 00:00:00 EST - P-0Y0M30DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081_negative__2: SUB: 2010-03-01 00:00:00 EST - P-0Y1M0DT0H0M0S = **2010-04-01 00:00:00 EDT**
test_bug_49081_negative__3: SUB: 2010-03-31 00:00:00 EDT - P-0Y0M1DT0H0M0S = **2010-04-01 00:00:00 EDT**
test_bug_49081_negative__4: SUB: 2010-03-31 00:00:00 EDT - P-0Y0M29DT0H0M0S = **2010-04-29 00:00:00 EDT**
test_bug_49081_negative__5: SUB: 2010-03-31 00:00:00 EDT - P-0Y0M30DT0H0M0S = **2010-04-30 00:00:00 EDT**
test_bug_49081_negative__6: SUB: 2010-03-30 00:00:00 EDT - P-0Y1M0DT0H0M0S = **2010-04-30 00:00:00 EDT**
test_bug_49081_negative__7: SUB: 2010-03-29 00:00:00 EDT - P-0Y1M1DT0H0M0S = **2010-04-30 00:00:00 EDT**
test_bug_49081_negative__8: SUB: 2010-01-01 00:00:00 EST - P-0Y0M28DT0H0M0S = **2010-01-29 00:00:00 EST**
test_bug_49081_negative__9: SUB: 2010-01-01 00:00:00 EST - P-0Y0M29DT0H0M0S = **2010-01-30 00:00:00 EST**
test_bug_49081_negative__10: SUB: 2010-01-01 00:00:00 EST - P-0Y0M30DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__11: SUB: 2010-01-01 00:00:00 EST - P-0Y1M0DT0H0M0S = **2010-02-01 00:00:00 EST**
test_bug_49081_negative__12: SUB: 2010-01-31 00:00:00 EST - P-0Y0M1DT0H0M0S = **2010-02-01 00:00:00 EST**
test_bug_49081_negative__13: SUB: 2010-01-31 00:00:00 EST - P-0Y0M27DT0H0M0S = **2010-02-27 00:00:00 EST**
test_bug_49081_negative__14: SUB: 2010-01-31 00:00:00 EST - P-0Y0M28DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__15: SUB: 2010-01-30 00:00:00 EST - P-0Y0M29DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__16: SUB: 2010-01-29 00:00:00 EST - P-0Y0M30DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__17: SUB: 2010-01-28 00:00:00 EST - P-0Y1M0DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__18: SUB: 2010-01-27 00:00:00 EST - P-0Y1M1DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__19: SUB: 2010-01-01 00:00:00 EST - P-0Y2M0DT0H0M0S = **2010-03-01 00:00:00 EST**
test_bug_49081_negative__20: SUB: 2010-01-31 00:00:00 EST - P-0Y1M1DT0H0M0S = **2010-03-04 00:00:00 EST**
test_bug_49081_negative__21: SUB: 2010-01-31 00:00:00 EST - P-0Y1M27DT0H0M0S = **2010-03-30 00:00:00 EDT**
test_bug_49081_negative__22: SUB: 2010-01-31 00:00:00 EST - P-0Y1M28DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081_negative__23: SUB: 2010-01-31 00:00:00 EST - P-0Y1M29DT0H0M0S = **2010-04-01 00:00:00 EDT**
test_bug_49081_negative__24: SUB: 2010-01-31 00:00:00 EST - P-0Y1M30DT0H0M0S = **2010-04-02 00:00:00 EDT**
test_bug_49081_negative__25: SUB: 2010-01-31 00:00:00 EST - P-0Y2M0DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081_negative__26: SUB: 2010-01-30 00:00:00 EST - P-0Y2M1DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081_negative__27: SUB: 2009-01-01 00:00:00 EST - P-0Y0M30DT0H0M0S = **2009-01-31 00:00:00 EST**
test_bug_49081_negative__28: SUB: 2010-02-28 00:00:00 EST - P-0Y0M27DT0H0M0S = **2010-03-27 00:00:00 EDT**
test_bug_49081_negative__29: SUB: 2010-02-28 00:00:00 EST - P-0Y1M0DT0H0M0S = **2010-03-28 00:00:00 EDT**
test_bug_49081_negative__30: SUB: 2010-02-28 00:00:00 EST - P-0Y1M1DT0H0M0S = **2010-03-29 00:00:00 EDT**
test_bug_49081_negative__31: SUB: 2010-02-27 00:00:00 EST - P-0Y1M0DT0H0M0S = **2010-03-27 00:00:00 EDT**
test_bug_49081_negative__32: SUB: 2010-02-26 00:00:00 EST - P-0Y1M1DT0H0M0S = **2010-03-27 00:00:00 EDT**"#,
    );
}

/// ext/date/tests/bug-gh11368.phpt
#[test]
fn bug_gh11368() {
    let mut datetime = DT::new("2023-06-04", "UTC");
    datetime.set_time(1, 1, 1, 1);
    let mut out = datetime.var_dump("DateTime", 1);
    assert!(datetime.modify("-100 ms"));
    out += &datetime.var_dump("DateTime", 1);
    assert_expectf(
        &out,
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-06-04 01:01:01.000001"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-06-04 01:01:00.900001"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#,
    );
}

/// ext/date/tests/bug13142.phpt
#[test]
fn bug13142() {
    let z = "US/Eastern";
    let mut out = String::new();
    for s in ["Sep 04 16:39:45 2001", "Sep 04 2001 16:39:45"] {
        out += &date("r\n", strtotime_abs(s, z).unwrap(), z);
    }
    assert_text(
        &out,
        r#"Tue, 04 Sep 2001 16:39:45 -0400
Tue, 04 Sep 2001 16:39:45 -0400"#,
    );
}

/// ext/date/tests/bug21966.phpt
#[test]
fn bug21966() {
    let z = "Europe/London";
    let out = format!(
        "27/3/04 = {}\n28/3/04 = {}\n28/3/04 = {}\n29/3/04 = {}\n30/3/04 = {}\n",
        mktime(0, 0, 0, 3, 27, 2004, z),
        mktime(0, 0, 0, 3, 28, 2004, z),
        mktime(2, 0, 0, 3, 28, 2004, z),
        mktime(0, 0, 0, 3, 29, 2004, z),
        mktime(0, 0, 0, 3, 30, 2004, z)
    );
    assert_text(
        &out,
        r#"27/3/04 = 1080345600
28/3/04 = 1080432000
28/3/04 = 1080435600
29/3/04 = 1080514800
30/3/04 = 1080601200"#,
    );
}

/// ext/date/tests/bug27780.phpt
#[test]
fn bug27780() {
    let timezones = [
        "America/Chicago",
        "Europe/Amsterdam",
        "Asia/Jerusalem",
        "Asia/Singapore",
        "America/Sao_Paulo",
    ];
    let timestrings = [
        "2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +59 seconds",
        "2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +60 seconds",
        "2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +61 seconds",
        "2004-04-07 00:00:00 -21 days",
        "2004-04-07 00:00:00 11 days ago",
        "2004-04-07 00:00:00 -10 day +2 hours",
        "2004-04-07 00:00:00 -1 day",
        "2004-04-07 00:00:00",
        "2004-04-07 00:00:00 +1 hour",
        "2004-04-07 00:00:00 +2 hour",
        "2004-04-07 00:00:00 +1 day",
        "2004-04-07 00:00:00 1 day",
        "2004-04-07 00:00:00 +21 days",
    ];
    let mut out = String::new();
    for z in timezones {
        let _ = writeln!(out, "{z}");
        for s in timestrings {
            let t = strtotime_abs(s, z).unwrap();
            let _ = writeln!(out, "{t}{} [{s}]", date(" [Y-m-d H:i:s T]", t, z));
        }
        out += "\n";
    }
    assert_expectf(
        &out,
        r#"America/Chicago
1076824799 [2004-02-14 23:59:59 CST] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +59 seconds]
1076824800 [2004-02-15 00:00:00 CST] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +60 seconds]
1076824801 [2004-02-15 00:00:01 CST] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +61 seconds]
1079503200 [2004-03-17 00:00:00 CST] [2004-04-07 00:00:00 -21 days]
1080367200 [2004-03-27 00:00:00 CST] [2004-04-07 00:00:00 11 days ago]
1080460800 [2004-03-28 02:00:00 CST] [2004-04-07 00:00:00 -10 day +2 hours]
1081227600 [2004-04-06 00:00:00 CDT] [2004-04-07 00:00:00 -1 day]
1081314000 [2004-04-07 00:00:00 CDT] [2004-04-07 00:00:00]
1081317600 [2004-04-07 01:00:00 CDT] [2004-04-07 00:00:00 +1 hour]
1081321200 [2004-04-07 02:00:00 CDT] [2004-04-07 00:00:00 +2 hour]
1081400400 [2004-04-08 00:00:00 CDT] [2004-04-07 00:00:00 +1 day]
1081400400 [2004-04-08 00:00:00 CDT] [2004-04-07 00:00:00 1 day]
1083128400 [2004-04-28 00:00:00 CDT] [2004-04-07 00:00:00 +21 days]

Europe/Amsterdam
1076799599 [2004-02-14 23:59:59 CET] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +59 seconds]
1076799600 [2004-02-15 00:00:00 CET] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +60 seconds]
1076799601 [2004-02-15 00:00:01 CET] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +61 seconds]
1079478000 [2004-03-17 00:00:00 CET] [2004-04-07 00:00:00 -21 days]
1080342000 [2004-03-27 00:00:00 CET] [2004-04-07 00:00:00 11 days ago]
1080435600 [2004-03-28 03:00:00 CEST] [2004-04-07 00:00:00 -10 day +2 hours]
1081202400 [2004-04-06 00:00:00 CEST] [2004-04-07 00:00:00 -1 day]
1081288800 [2004-04-07 00:00:00 CEST] [2004-04-07 00:00:00]
1081292400 [2004-04-07 01:00:00 CEST] [2004-04-07 00:00:00 +1 hour]
1081296000 [2004-04-07 02:00:00 CEST] [2004-04-07 00:00:00 +2 hour]
1081375200 [2004-04-08 00:00:00 CEST] [2004-04-07 00:00:00 +1 day]
1081375200 [2004-04-08 00:00:00 CEST] [2004-04-07 00:00:00 1 day]
1083103200 [2004-04-28 00:00:00 CEST] [2004-04-07 00:00:00 +21 days]

Asia/Jerusalem
1076795999 [2004-02-14 23:59:59 IST] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +59 seconds]
1076796000 [2004-02-15 00:00:00 IST] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +60 seconds]
1076796001 [2004-02-15 00:00:01 IST] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +61 seconds]
1079474400 [2004-03-17 00:00:00 IST] [2004-04-07 00:00:00 -21 days]
1080338400 [2004-03-27 00:00:00 IST] [2004-04-07 00:00:00 11 days ago]
1080432000 [2004-03-28 02:00:00 IST] [2004-04-07 00:00:00 -10 day +2 hours]
1081202400 [2004-04-06 00:00:00 IST] [2004-04-07 00:00:00 -1 day]
1081288800 [2004-04-07 00:00:00 IST] [2004-04-07 00:00:00]
1081292400 [2004-04-07 02:00:00 IDT] [2004-04-07 00:00:00 +1 hour]
1081292400 [2004-04-07 02:00:00 IDT] [2004-04-07 00:00:00 +2 hour]
1081371600 [2004-04-08 00:00:00 IDT] [2004-04-07 00:00:00 +1 day]
1081371600 [2004-04-08 00:00:00 IDT] [2004-04-07 00:00:00 1 day]
1083099600 [2004-04-28 00:00:00 IDT] [2004-04-07 00:00:00 +21 days]

Asia/Singapore
1076774399 [2004-02-14 23:59:59 %s] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +59 seconds]
1076774400 [2004-02-15 00:00:00 %s] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +60 seconds]
1076774401 [2004-02-15 00:00:01 %s] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +61 seconds]
1079452800 [2004-03-17 00:00:00 %s] [2004-04-07 00:00:00 -21 days]
1080316800 [2004-03-27 00:00:00 %s] [2004-04-07 00:00:00 11 days ago]
1080410400 [2004-03-28 02:00:00 %s] [2004-04-07 00:00:00 -10 day +2 hours]
1081180800 [2004-04-06 00:00:00 %s] [2004-04-07 00:00:00 -1 day]
1081267200 [2004-04-07 00:00:00 %s] [2004-04-07 00:00:00]
1081270800 [2004-04-07 01:00:00 %s] [2004-04-07 00:00:00 +1 hour]
1081274400 [2004-04-07 02:00:00 %s] [2004-04-07 00:00:00 +2 hour]
1081353600 [2004-04-08 00:00:00 %s] [2004-04-07 00:00:00 +1 day]
1081353600 [2004-04-08 00:00:00 %s] [2004-04-07 00:00:00 1 day]
1083081600 [2004-04-28 00:00:00 %s] [2004-04-07 00:00:00 +21 days]

America/Sao_Paulo
1076810399 [2004-02-14 23:59:59 %s] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +59 seconds]
1076814000 [2004-02-15 00:00:00 %s] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +60 seconds]
1076814001 [2004-02-15 00:00:01 %s] [2004-04-07 00:00:00 -2 months +7 days +23 hours +59 minutes +61 seconds]
1079492400 [2004-03-17 00:00:00 %s] [2004-04-07 00:00:00 -21 days]
1080356400 [2004-03-27 00:00:00 %s] [2004-04-07 00:00:00 11 days ago]
1080450000 [2004-03-28 02:00:00 %s] [2004-04-07 00:00:00 -10 day +2 hours]
1081220400 [2004-04-06 00:00:00 %s] [2004-04-07 00:00:00 -1 day]
1081306800 [2004-04-07 00:00:00 %s] [2004-04-07 00:00:00]
1081310400 [2004-04-07 01:00:00 %s] [2004-04-07 00:00:00 +1 hour]
1081314000 [2004-04-07 02:00:00 %s] [2004-04-07 00:00:00 +2 hour]
1081393200 [2004-04-08 00:00:00 %s] [2004-04-07 00:00:00 +1 day]
1081393200 [2004-04-08 00:00:00 %s] [2004-04-07 00:00:00 1 day]
1083121200 [2004-04-28 00:00:00 %s] [2004-04-07 00:00:00 +21 days]"#,
    );
}

/// ext/date/tests/bug29595.phpt
#[test]
fn bug29595() {
    assert_eq!(
        strtotime_abs("2004-08-09 14:48:27.304809+10", "GMT"),
        Some(1092026907)
    );
}

/// ext/date/tests/bug32588.phpt
#[test]
fn bug32588() {
    let z = "America/New_York";
    let mut out = String::new();
    for s in ["last saturday", "last sunday", "last monday"] {
        out += &date("D Y/m/d/H:i:s", strtotime(s, 1112703348, z).unwrap(), z);
        out += "\n";
    }
    assert_text(
        &out,
        r#"Sat 2005/04/02/00:00:00
Sun 2005/04/03/00:00:00
Mon 2005/04/04/00:00:00"#,
    );
}

/// ext/date/tests/bug33452.phpt
#[test]
fn bug33452() {
    let z = "GMT";
    let t = strtotime_abs("2005-1-1", z).unwrap();
    let out = format!("{}\n{}\n", date("Y-W", t, z), date("o-W", t, z));
    assert_text(
        &out,
        r#"2005-53
2004-53"#,
    );
}

/// ext/date/tests/bug33869.phpt
#[test]
fn bug33869() {
    let z = "UTC";
    let tm = strtotime_abs("2005-01-01 01:01:01", z).unwrap();
    let mut out = String::new();
    for s in [
        "+5days", "+1month", "+1year", "+5 days", "+1 month", "+1 year",
    ] {
        out += &date(DATE_ISO8601, strtotime(s, tm, z).unwrap(), z);
        out += "\n";
    }
    assert_text(
        &out,
        r#"2005-01-06T01:01:01+0000
2005-02-01T01:01:01+0000
2006-01-01T01:01:01+0000
2005-01-06T01:01:01+0000
2005-02-01T01:01:01+0000
2006-01-01T01:01:01+0000"#,
    );
}

/// ext/date/tests/bug35499.phpt
#[test]
fn bug35499() {
    let z = "UTC";
    let mut out = String::new();
    for s in ["11/20/2005 8:00 AM \r\n", "  11/20/2005 8:00 AM \r\n"] {
        out += &date(DATE_ISO8601, strtotime_abs(s, z).unwrap(), z);
        out += "\n";
    }
    out += &date_parse(" a ").var_dump();
    out += &date_parse(" \n ").var_dump();
    assert_text(
        &out,
        r#"2005-11-20T08:00:00+0000
2005-11-20T08:00:00+0000
array(16) {
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
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(2)
  ["zone"]=>
  int(3600)
  ["is_dst"]=>
  bool(false)
  ["tz_abbr"]=>
  string(1) "A"
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
}"#,
    );
}

/// ext/date/tests/bug35887.phpt
#[test]
fn bug35887() {
    let z = "UTC";
    assert_eq!(
        date(
            DATE_ISO8601,
            strtotime_abs("2006-1-6T0:0:0-8:0", z).unwrap(),
            z
        ),
        "2006-01-06T08:00:00+0000"
    );
}

/// ext/date/tests/bug37368.phpt
#[test]
fn bug37368() {
    let z = "UTC";
    assert_eq!(
        date(
            "r",
            strtotime_abs("Mon, 08 May 2006 13:06:44 -0400 +30 days", z).unwrap(),
            z
        ),
        "Wed, 07 Jun 2006 17:06:44 +0000"
    );
}

/// ext/date/tests/bug40743.phpt
#[test]
fn bug40743() {
    let mut out = String::new();
    let dt = DT::new_in("@1200506699", "Europe/Berlin", "UTC");
    let _ = writeln!(out, "{}", dt.format(DATE_RFC822));
    let _ = writeln!(out, "{}", dt.format("T e Z"));
    out += "-----\n";

    let z = "America/New_York";
    let f = format!("{DATE_RFC822} e T O U");
    let _ = writeln!(out, "{}", DT::new("16 Jan 08 13:04:59", z).format(&f));
    let _ = writeln!(out, "{}", DT::new("@1200506699", z).format(&f));
    let mut dt = DT::new("@1200506699", z);
    dt.set_timezone("America/New_York");
    let _ = writeln!(out, "{}", dt.format(&f));
    let _ = writeln!(
        out,
        "{}",
        DT::new_in("@1200506699", "Europe/Berlin", z).format(&f)
    );
    let _ = writeln!(
        out,
        "{}",
        DT::new("16 Jan 08 13:04:59 America/Chicago", z).format(&f)
    );
    let _ = writeln!(
        out,
        "{}",
        DT::new_in("16 Jan 08 13:04:59 America/Chicago", "Europe/Berlin", z).format(&f)
    );
    assert_text(
        &out,
        r#"Wed, 16 Jan 08 18:04:59 +0000
GMT+0000 +00:00 0
-----
Wed, 16 Jan 08 13:04:59 -0500 America/New_York EST -0500 1200506699
Wed, 16 Jan 08 18:04:59 +0000 +00:00 GMT+0000 +0000 1200506699
Wed, 16 Jan 08 13:04:59 -0500 America/New_York EST -0500 1200506699
Wed, 16 Jan 08 18:04:59 +0000 +00:00 GMT+0000 +0000 1200506699
Wed, 16 Jan 08 13:04:59 -0600 America/Chicago CST -0600 1200510299
Wed, 16 Jan 08 13:04:59 -0600 America/Chicago CST -0600 1200510299"#,
    );
}

/// ext/date/tests/bug41842.phpt
#[test]
fn bug41842() {
    let date = DT::new("-2007-06-28 00:00:00", "UTC");
    assert_eq!(date.format(DATE_ISO8601), "-2007-06-28T00:00:00+0000");
}

/// ext/date/tests/bug43452.phpt
#[test]
fn bug43452() {
    let z = "Europe/Oslo";
    let groups = [
        [
            "Thursday Nov 2007",
            "1 Thursday Nov 2007",
            "2 Thursday Nov 2007",
            "3 Thursday Nov 2007",
        ],
        [
            "Thursday Nov 2007",
            "+1 week Thursday Nov 2007",
            "+2 week Thursday Nov 2007",
            "+3 week Thursday Nov 2007",
        ],
        [
            "Thursday Nov 2007",
            "first Thursday Nov 2007",
            "second Thursday Nov 2007",
            "third Thursday Nov 2007",
        ],
        [
            "Friday Nov 2007",
            "first Friday Nov 2007",
            "second Friday Nov 2007",
            "third Friday Nov 2007",
        ],
    ];
    let mut out = String::new();
    for g in groups {
        for s in g {
            out += &date("Y-m-d\\TH:i:sO", strtotime_abs(s, z).unwrap(), z);
            out += "\n";
        }
        out += "\n";
    }
    assert_text(
        &out,
        r#"2007-11-01T00:00:00+0100
2007-11-01T00:00:00+0100
2007-11-08T00:00:00+0100
2007-11-15T00:00:00+0100

2007-11-01T00:00:00+0100
2007-11-08T00:00:00+0100
2007-11-15T00:00:00+0100
2007-11-22T00:00:00+0100

2007-11-01T00:00:00+0100
2007-11-08T00:00:00+0100
2007-11-15T00:00:00+0100
2007-11-22T00:00:00+0100

2007-11-02T00:00:00+0100
2007-11-02T00:00:00+0100
2007-11-09T00:00:00+0100
2007-11-16T00:00:00+0100"#,
    );
}

/// ext/date/tests/bug44780.phpt
#[test]
fn bug44780() {
    assert_eq!(
        php_timelib::timezone_id_from_abbr("", (5.5 * 3600.0) as i64, 0),
        Some("Asia/Kolkata")
    );
    assert_eq!(
        php_timelib::timezone_id_from_abbr("", 28800, 0),
        Some("Asia/Shanghai")
    );
}

/// ext/date/tests/bug45866.phpt
#[test]
fn bug45866() {
    let z = "UTC";
    let mut out = String::new();
    let mut d = DT::new("2009-07-29 16:44:23 Europe/London", z);
    assert!(d.modify("+1.61538461538 day"));
    let _ = writeln!(out, "{}", d.format("r"));

    let mut d = DT::new("2009-07-29 16:44:23 Europe/London", z);
    assert!(d.modify("61538461538 day"));
    let _ = writeln!(out, "{}", d.format("r"));

    let mut d = DT::new("2009-07-29 16:44:23 Europe/London", z);
    let s = "£61538461538 day";
    match php::date_modify(&mut d.t, s.as_bytes()) {
        Ok(_) => out += "no exception\n",
        Err(e) => {
            let m = &e.errors[0];
            let _ = writeln!(
                out,
                "DateMalformedStringException: DateTime::modify(): Failed to parse time string ({s}) at position {} (?): {}",
                m.position, m.message
            );
        }
    }
    let _ = writeln!(out, "{}", d.format("r"));
    assert_expectf(
        &out,
        r#"Thu, 14 Aug 168488594 16:44:23 +0100
Thu, 14 Aug 168488594 16:44:23 +0100
DateMalformedStringException: DateTime::modify(): Failed to parse time string (£61538461538 day) at position 0 (%s): Unexpected character
Wed, 29 Jul 2009 16:44:23 +0100"#,
    );
}

/// ext/date/tests/bug48097.phpt
#[test]
fn bug48097() {
    let mut d = DT::new("Sun, 22 May 1955 02:00:00 +0200", "UTC");
    let mut out = d.var_dump("DateTime", 1);
    let _ = write!(out, "{}\n{}\n\n", d.format(DATE_ISO8601), d.format("U"));
    d.set_timezone("Europe/Budapest");
    out += &d.var_dump("DateTime", 1);
    let _ = write!(out, "{}\n\n{}\n\n", d.format(DATE_ISO8601), d.format("U"));
    assert_text(
        &out,
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1955-05-22 02:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+02:00"
}
1955-05-22T02:00:00+0200
-461203200

object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1955-05-22 01:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(15) "Europe/Budapest"
}
1955-05-22T01:00:00+0100

-461203200"#,
    );
}

/// ext/date/tests/bug49585.phpt
#[test]
fn bug49585() {
    let mut date = DT::new("-1500-01-01", "UTC");
    let mut out = dump_str(&date.format("r"));
    date.set_date(-2147483648, 1, 1);
    out += &dump_str(&date.format("r"));
    out += &dump_str(&date.format("c"));
    assert_text(
        &out,
        r#"string(32) "Fri, 01 Jan -1500 00:00:00 +0000"
string(38) "Tue, 01 Jan -2147483648 00:00:00 +0000"
string(32) "-2147483648-01-01T00:00:00+00:00""#,
    );
}

/// ext/date/tests/bug50475.phpt
#[test]
fn bug50475() {
    let mut date = DT::new("18-01-2009 00:00:00", "Asia/Calcutta");
    date.set_isodate(2009, 6, 1);
    let mut out = dump_str(&date.format("Y-m-d H:i:s"));
    date.set_time(8, 0, 0, 0);
    out += &dump_str(&date.format("Y-m-d H:i:s"));
    assert_text(
        &out,
        r#"string(19) "2009-02-02 00:00:00"
string(19) "2009-02-02 08:00:00""#,
    );
}

/// ext/date/tests/bug51934.phpt
#[test]
fn bug51934() {
    let z = "America/Los_Angeles";
    let mut out = String::new();
    for s in [
        "2010-05-27 19:18 4 Sunday ago",
        "2010-05-27 19:18 4 Sundays ago",
    ] {
        out += &date("Y-m-d H:i:s", strtotime_abs(s, z).unwrap(), z);
        out += "\n";
    }
    assert_expectf(
        &out,
        r#"2010-05-02 19:18:00
2010-05-02 19:18:00"#,
    );
}

/// ext/date/tests/bug52113.phpt (partial: the diff and the DatePeriod built from it;
/// serialisation, var_export and the __set_state() interval are not ported)
#[test]
fn bug52113() {
    let start = DT::new("2003-01-02 08:00:00", "UTC");
    let end = DT::new("2003-01-02 12:00:00", "UTC");
    let diff = start.diff(&end);
    let p = String::from("object(DatePeriod)#5 (7) {\n")
        + &period_props_dump("DateTime", &start, None, &diff, 2 + 1, true, false)
        + "}\n";
    assert_expectf(
        &p,
        r#"object(DatePeriod)#%d (%d) {
  ["start"]=>
  object(DateTime)#%d (%d) {
    ["date"]=>
    string(26) "2003-01-02 08:00:00.000000"
    ["timezone_type"]=>
    int(3)
    ["timezone"]=>
    string(3) "UTC"
  }
  ["current"]=>
  NULL
  ["end"]=>
  NULL
  ["interval"]=>
  object(DateInterval)#%d (%d) {
    ["y"]=>
    int(0)
    ["m"]=>
    int(0)
    ["d"]=>
    int(0)
    ["h"]=>
    int(4)
    ["i"]=>
    int(0)
    ["s"]=>
    int(0)
    ["f"]=>
    float(0)
    ["invert"]=>
    int(0)
    ["days"]=>
    int(0)
    ["from_string"]=>
    bool(false)
  }
  ["recurrences"]=>
  int(3)
  ["include_start_date"]=>
  bool(true)
  ["include_end_date"]=>
  bool(false)
}"#,
    );
    assert_expectf(
        &interval_var_dump(&diff, 3),
        r#"object(DateInterval)#%d (%d) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(0)
  ["h"]=>
  int(4)
  ["i"]=>
  int(0)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  int(0)
  ["from_string"]=>
  bool(false)
}"#,
    );
}

/// ext/date/tests/bug52577.phpt
#[test]
fn bug52577() {
    let z = "Europe/Kiev";
    let s = "7.8.2010";
    let unixtime = strtotime_abs(s, z).unwrap();
    let date = DT::new(&format!("@{unixtime}"), z);
    let out = format!(
        "String: {s}\nUnixtime: {unixtime}\nDate(PHP): {}\nDateTime(PHP Class): {}",
        common::date(DATE_RFC2822, unixtime, z),
        date.format(DATE_RFC2822)
    );
    assert_text(
        &out,
        r#"String: 7.8.2010
Unixtime: 1281128400
Date(PHP): Sat, 07 Aug 2010 00:00:00 +0300
DateTime(PHP Class): Fri, 06 Aug 2010 21:00:00 +0000"#,
    );
}

/// ext/date/tests/bug53437_var2.phpt (partial: the original interval; the unserialised
/// copy is not ported)
#[test]
fn bug53437_var2() {
    let di0 = php::date_interval_create("P2Y4DT6H8M").unwrap();
    assert_expectf(
        &interval_var_dump(&di0, 1),
        r#"object(DateInterval)#1 (%d) {
  ["y"]=>
  int(2)
  ["m"]=>
  int(0)
  ["d"]=>
  int(4)
  ["h"]=>
  int(6)
  ["i"]=>
  int(8)
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
}

/// ext/date/tests/bug53879.phpt
#[test]
fn bug53879() {
    let date = DT::from_format(DATE_COOKIE, "Mon, 21-Jan-2041 15:24:52 GMT", None, "UTC").unwrap();
    assert_text(
        &V::Obj("DateTime".into(), 1, date.props()).print_r(),
        r#"DateTime Object
(
    [date] => 2041-01-21 15:24:52.000000
    [timezone_type] => 2
    [timezone] => GMT
)"#,
    );
}

/// ext/date/tests/bug55253.phpt
#[test]
fn bug55253() {
    let z = "America/New_York";
    let f = "Y-m-d H:i:s T";
    let interval = php::date_interval_create("PT2H1M").unwrap();
    let mut out = String::new();

    let mut date3 = DT::new("2010-10-04 02:18:48", z);
    let mut date2 = DT::new("2010-10-04 02:18:48 EDT", z);
    let _ = writeln!(out, "Zone Type 3: {}", date3.format(f));
    let _ = writeln!(out, "Zone Type 2: {}", date2.format(f));
    let _ = writeln!(
        out,
        "{}",
        interval_format(&interval, "Add %h hours %i minutes")
    );
    date3.add_interval(&interval);
    date2.add_interval(&interval);
    let _ = writeln!(out, "Zone Type 3: {}", date3.format(f));
    let _ = writeln!(out, "Zone Type 2: {}", date2.format(f));

    let mut date3 = DT::new("2010-10-04 04:19:48", z);
    let mut date2 = DT::new("2010-10-04 04:19:48 EDT", z);
    let _ = writeln!(
        out,
        "{}",
        interval_format(&interval, "Subtract %h hours %i minutes from expected")
    );
    date3.sub_interval(&interval);
    date2.sub_interval(&interval);
    let _ = writeln!(out, "Zone Type 3: {}", date3.format(f));
    let _ = writeln!(out, "Zone Type 2: {}", date2.format(f));
    assert_text(
        &out,
        r#"Zone Type 3: 2010-10-04 02:18:48 EDT
Zone Type 2: 2010-10-04 02:18:48 EDT
Add 2 hours 1 minutes
Zone Type 3: 2010-10-04 04:19:48 EDT
Zone Type 2: 2010-10-04 04:19:48 EDT
Subtract 2 hours 1 minutes from expected
Zone Type 3: 2010-10-04 02:18:48 EDT
Zone Type 2: 2010-10-04 02:18:48 EDT"#,
    );
}

/// ext/date/tests/bug60774.phpt (partial: the format() lines; the var_dump shows
/// PHP-level from_string/date_string properties)
#[test]
fn bug60774() {
    let i = php::date_interval_create_from_date_string("2 days").unwrap();
    assert_eq!(interval_format(&i, "%d"), "2");
    assert_eq!(interval_format(&i, "%a"), "(unknown)");
}

/// ext/date/tests/bug62852_var2.phpt (partial: whether the stored properties are
/// accepted by `php_date_initialize_from_hash()`; __wakeup is not ported)
#[test]
fn bug62852_var2() {
    assert!(date_from_hash("10007-06-07 03:51:49", 3, "UTC", "GMT").is_none());
}

/// ext/date/tests/bug64157.phpt
#[test]
fn bug64157() {
    assert_eq!(
        last_error_at("s", "0", 0),
        "A two digit second could not be found"
    );
}

/// ext/date/tests/bug66721.phpt (partial: whether the stored properties are accepted by
/// `php_date_initialize_from_hash()`)
#[test]
fn bug66721() {
    assert!(date_from_hash("2014-02-15 02:00:51", 3, "1234567890", "UTC").is_none());
}

/// ext/date/tests/bug67251.phpt
#[test]
fn bug67251() {
    assert_text(
        &date_parse_from_format("\\", "AAAABBBB").var_dump(),
        r#"array(12) {
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
  array(1) {
    [0]=>
    string(13) "Trailing data"
  }
  ["is_localtime"]=>
  bool(false)
}"#,
    );
}

/// ext/date/tests/bug68503.phpt
#[test]
fn bug68503() {
    let mut out = String::new();
    for z in ["Europe/London", "UTC"] {
        let d = DT::new("2015-02-01", z).diff(&DT::new("2015-05-01", z));
        let _ = writeln!(out, "{}", interval_format(&d, "%yY %mM %dD"));
    }
    assert_text(
        &out,
        r#"0Y 3M 0D
0Y 3M 0D"#,
    );
}

/// ext/date/tests/bug69089.phpt
#[test]
fn bug69089() {
    let date = DT::new("2009-09-28 09:45:31.918312", "America/Buenos_Aires");
    let out = dump_str(&date.format(DATE_RFC3339_EXTENDED))
        + &dump_str(&date.format("u"))
        + &dump_str(&date.format("v"));
    assert_text(
        &out,
        r#"string(29) "2009-09-28T09:45:31.918-03:00"
string(6) "918312"
string(3) "918""#,
    );
}

/// ext/date/tests/bug71826.phpt
#[test]
fn bug71826() {
    let z = "Asia/Tokyo";
    let mut out = String::new();
    for (i, (a, b)) in [
        ("2015-2-1", "2015-3-1"),
        ("2015-3-1", "2015-3-29"),
        ("2015-4-1", "2015-4-29"),
    ]
    .into_iter()
    .enumerate()
    {
        let d = DT::new(a, z).diff(&DT::new(b, z));
        let label = ["a", "b", "c"][i];
        if i > 0 {
            out += "\n";
        }
        let _ = writeln!(out, "{label}(Asia/Tokyo): {a} <--> {b}");
        out += &dump_int(d.diff.m);
        out += &dump_int(d.diff.d);
    }
    assert_text(
        &out,
        r#"a(Asia/Tokyo): 2015-2-1 <--> 2015-3-1
int(1)
int(0)

b(Asia/Tokyo): 2015-3-1 <--> 2015-3-29
int(0)
int(28)

c(Asia/Tokyo): 2015-4-1 <--> 2015-4-29
int(0)
int(28)"#,
    );
}

/// ext/date/tests/bug74524.phpt
#[test]
fn bug74524() {
    let z = "Europe/Amsterdam";
    let a = DT::new("2017-11-17 22:05:26.000000", z);
    let b = DT::new("2017-04-03 22:29:15.079459", z);
    let diff = a.diff(&b);
    assert_text(
        &V::Obj("DateInterval".into(), 3, interval_props(&diff)).print_r(),
        r#"DateInterval Object
(
    [y] => 0
    [m] => 7
    [d] => 13
    [h] => 23
    [i] => 36
    [s] => 10
    [f] => 0.920541
    [invert] => 1
    [days] => 227
    [from_string] => 
)"#,
    );
}

/// ext/date/tests/bug75035.phpt (serialize()/unserialize() emulated with the object
/// properties and `php_date_initialize_from_hash()`; the PHP_VERSION lines are not
/// ported)
#[test]
fn bug75035() {
    let mut mismatches = Vec::new();
    for extreme in [i64::MIN, i64::MAX] {
        for i in (1..64).rev() {
            let v = extreme >> i;
            let d = DT::new(&format!("@{v}"), "UTC");
            let original = d.format("Y-m-d H:i:s");
            let rt = match date_roundtrip(&d) {
                Some(u) => u.format("Y-m-d H:i:s"),
                None => format!("failed unserialization: {:?}", d.props()),
            };
            if original != rt {
                mismatches.push(format!("[{v}] {original} => {rt}"));
            }
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

/// ext/date/tests/bug75857.phpt (the output only depends on the zone, not on "now")
#[test]
fn bug75857() {
    let mut out = String::new();
    for z in [
        "America/Argentina/ComodRivadavia",
        "America/Indiana/Indianapolis",
        "America/Sao_Paulo",
    ] {
        out += &dump_str(&DT::new_in("now", z, "UTC").format("e"));
    }
    assert_text(
        &out,
        r#"string(32) "America/Argentina/ComodRivadavia"
string(28) "America/Indiana/Indianapolis"
string(17) "America/Sao_Paulo""#,
    );
}

/// ext/date/tests/bug77097.phpt
#[test]
fn bug77097() {
    let now = DT::new("2018-11-03 11:34:20.781751", "UTC");
    let ago = DT::new("2018-11-03 11:34:20.000000", "UTC");
    let mut out = String::new();
    for (a, b, absolute) in [
        (&now, &ago, false),
        (&ago, &now, false),
        (&now, &ago, true),
        (&ago, &now, true),
    ] {
        let d = php::date_diff(&a.t, &b.t, absolute);
        out += &dump_int(i64::from(d.diff.invert));
        out += &dump_int(d.diff.s);
        out += &V::Float(d.diff.us as f64 / 1_000_000.0).var_dump();
    }
    assert_text(
        &out,
        r#"int(1)
int(0)
float(0.781751)
int(0)
int(0)
float(0.781751)
int(0)
int(0)
float(0.781751)
int(0)
int(0)
float(0.781751)"#,
    );
}

/// ext/date/tests/bug78383.phpt
#[test]
fn bug78383() {
    let out = V::Arr(DT::new("2000-01-01 UTC", "UTC").props()).var_dump()
        + &V::Arr(tz_props(&php::timezone_open("Europe/Berlin").unwrap())).var_dump();
    assert_text(
        &out,
        r#"array(3) {
  ["date"]=>
  string(26) "2000-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
array(2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/Berlin"
}"#,
    );
}

/// ext/date/tests/bug79580.phpt
#[test]
fn bug79580() {
    let mut out = String::new();
    for value in ["31 2020", "60 2020", "91 2020", "121 2020", "130 2020"] {
        let _ = write!(out, "Testing for {value}: ");
        match DT::from_format("z Y", value, None, "UTC") {
            Some(dt) => out += &dt.format("Y-m-d"),
            None => out += &last_error_at("z Y", value, 0),
        }
        out += "\n";
    }
    assert_text(
        &out,
        r#"Testing for 31 2020: A 'day of year' can only come after a year has been found
Testing for 60 2020: A 'day of year' can only come after a year has been found
Testing for 91 2020: A 'day of year' can only come after a year has been found
Testing for 121 2020: A 'day of year' can only come after a year has been found
Testing for 130 2020: A 'day of year' can only come after a year has been found"#,
    );
}

/// ext/date/tests/bug80664.phpt
#[test]
fn bug80664() {
    let line = |dt: &DT| format!("{} | {}\n", dt.format("Y-m-d H:i:s T"), dt.timestamp());
    let mut out = String::new();
    let mut dt = DT::new("@1604215800", "UTC");
    dt.set_timezone("America/Boise");
    out += &line(&dt);
    dt.add("PT1H");
    out += &line(&dt);
    dt.add("PT1H");
    out += &line(&dt);
    dt.add("PT1M");
    out += &line(&dt);
    out += "\n";

    let mut dt = DT::new("@1604219400", "UTC");
    dt.set_timezone("UTC");
    out += &line(&dt);
    dt.set_timezone("America/Boise");
    out += &line(&dt);
    assert_text(
        &out,
        r#"2020-11-01 01:30:00 MDT | 1604215800
2020-11-01 01:30:00 MST | 1604219400
2020-11-01 02:30:00 MST | 1604223000
2020-11-01 02:31:00 MST | 1604223060

2020-11-01 08:30:00 UTC | 1604219400
2020-11-01 01:30:00 MST | 1604219400"#,
    );
}

/// ext/date/tests/bug81106.phpt
#[test]
fn bug81106() {
    let mut date_interval = php::date_interval_create("PT0S").unwrap();
    date_interval.diff.us = (1.234_f64 * 1_000_000.0) as i64;
    let mut d = DT::new("2000-01-01 00:00:00", "UTC");
    d.add_interval(&date_interval);
    assert_eq!(d.format("Y-m-d H:i:s.u"), "2000-01-01 00:00:01.234000");
}

/// ext/date/tests/bug81565.phpt (the var_export() output is checked through the
/// object's properties)
#[test]
fn bug81565() {
    let d = DT::from_format("Y-m-d H:i:sO", "0021-08-21 00:00:00+00:49:56", None, "UTC").unwrap();
    let props: Vec<String> = d
        .props()
        .into_iter()
        .map(|(k, v)| match (k, v) {
            (K::S(k), V::Str(s)) => format!("   '{k}' => '{s}',"),
            (K::S(k), V::Int(i)) => format!("   '{k}' => {i},"),
            other => panic!("unexpected property {other:?}"),
        })
        .collect();
    let out = format!(
        "\\DateTime::__set_state(array(\n{}\n))\n{}",
        props.join("\n"),
        php::timezone_open("+01:45:30").unwrap().name()
    );
    assert_text(
        &out,
        r#"\DateTime::__set_state(array(
   'date' => '0021-08-21 00:00:00.000000',
   'timezone_type' => 1,
   'timezone' => '+00:49:56',
))
+01:45:30"#,
    );
}

/// ext/date/tests/date-lenient.phpt
#[test]
fn date_lenient() {
    let mut out = String::new();
    for f in ["m/d/y", "m/d/y+", "+m/d/y", "m/d/y++"] {
        out += &date_parse_from_format(f, "06/08/04 12:00").print_r();
    }
    for f in ["m/d/y+", "+m/d/y"] {
        out += &date_parse_from_format(f, "06/08/04").print_r();
    }
    assert_text(
        &out,
        r#"Array
(
    [year] => 2004
    [month] => 6
    [day] => 8
    [hour] => 
    [minute] => 
    [second] => 
    [fraction] => 
    [warning_count] => 0
    [warnings] => Array
        (
        )

    [error_count] => 1
    [errors] => Array
        (
            [8] => Trailing data
        )

    [is_localtime] => 
)
Array
(
    [year] => 2004
    [month] => 6
    [day] => 8
    [hour] => 
    [minute] => 
    [second] => 
    [fraction] => 
    [warning_count] => 1
    [warnings] => Array
        (
            [8] => Trailing data
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
    [hour] => 
    [minute] => 
    [second] => 
    [fraction] => 
    [warning_count] => 1
    [warnings] => Array
        (
            [8] => Trailing data
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
    [hour] => 
    [minute] => 
    [second] => 
    [fraction] => 
    [warning_count] => 1
    [warnings] => Array
        (
            [8] => Trailing data
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
    [hour] => 
    [minute] => 
    [second] => 
    [fraction] => 
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
    [hour] => 
    [minute] => 
    [second] => 
    [fraction] => 
    [warning_count] => 0
    [warnings] => Array
        (
        )

    [error_count] => 0
    [errors] => Array
        (
        )

    [is_localtime] => 
)"#,
    );
}

/// ext/date/tests/date_basic1.phpt
#[test]
fn date_basic1() {
    let z = "Europe/London";
    let timestamp = mktime(10, 44, 30, 2, 27, 2009, z);
    let mut out = String::from("*** Testing date() : basic functionality ***\n");
    for f in [
        "F j, Y, g:i a",
        "m.d.y",
        "j, n, Y",
        "Ymd",
        "h-i-s, j-m-y, it is w Day",
        "\\i\\t \\i\\s \\t\\h\\e jS \\d\\a\\y.",
        "D M j G:i:s T Y",
        "H:m:s \\m \\i\\s\\ \\m\\o\\n\\t\\h",
        "H:i:s",
    ] {
        out += &dump_str(&date(f, timestamp, z));
    }
    assert_text(
        &out,
        r#"*** Testing date() : basic functionality ***
string(27) "February 27, 2009, 10:44 am"
string(8) "02.27.09"
string(11) "27, 2, 2009"
string(8) "20090227"
string(39) "10-44-30, 27-02-09, 4428 4430 5 Friam09"
string(19) "it is the 27th day."
string(28) "Fri Feb 27 10:44:30 GMT 2009"
string(19) "10:02:30 m is month"
string(8) "10:44:30""#,
    );
}

/// ext/date/tests/date_create_from_format_basic2.phpt
#[test]
fn date_create_from_format_basic2() {
    assert!(DT::from_format("Y-m-d", "2009---01", None, "UTC").is_none());
}

/// ext/date/tests/date_default_timezone_set-1.phpt (an empty date.timezone falls back
/// to UTC; the startup warning is not ported)
#[test]
fn date_default_timezone_set_1() {
    let date1 = strtotime_abs("2005-01-12 08:00:00", "UTC").unwrap();
    let date2 = strtotime_abs("2005-07-12 08:00:00", "UTC").unwrap();
    let k = "America/Indiana/Knox";
    let date3 = strtotime_abs("2005-01-12 08:00:00", k).unwrap();
    let date4 = strtotime_abs("2005-07-12 08:00:00", k).unwrap();
    let mut out = format!("{k}\n");
    for t in [date1, date2, date3, date4] {
        out += &date(DATE_ISO8601, t, k);
        out += "\n";
    }
    assert_text(
        &out,
        "America/Indiana/Knox\n2005-01-12T03:00:00-0500\n2005-07-12T03:00:00-0500\n2005-01-12T08:00:00-0500\n2005-07-12T08:00:00-0500\n",
    );
}

/// ext/date/tests/date_interval_non_relative_warning.phpt (the check PHP applies to the
/// parsed time: `have_date || have_time || have_zone`)
#[test]
fn date_interval_non_relative_warning() {
    let formats = [
        "next weekday 15:30",
        "+5 hours noon",
        "-8 days March 23",
        "+72 seconds UTC",
    ];
    let non_relative = |f: &str| {
        let r = php_timelib::strtotime(f, &builtin_tzinfo);
        assert!(r.errors.errors.is_empty(), "{f}: {:?}", r.errors);
        r.time.have_date != 0 || r.time.have_time != 0 || r.time.have_zone != 0
    };
    let mut out = String::new();
    for f in formats {
        if non_relative(f) {
            let _ = writeln!(
                out,
                "DateMalformedIntervalStringException: String '{f}' contains non-relative elements"
            );
        }
    }
    out += "====\n";
    for f in formats {
        if non_relative(f) {
            let _ = writeln!(
                out,
                "\nWarning: date_interval_create_from_date_string(): String '{f}' contains non-relative elements in t.php on line 1"
            );
        }
    }
    assert_expectf(
        &out,
        r#"DateMalformedIntervalStringException: String 'next weekday 15:30' contains non-relative elements
DateMalformedIntervalStringException: String '+5 hours noon' contains non-relative elements
DateMalformedIntervalStringException: String '-8 days March 23' contains non-relative elements
DateMalformedIntervalStringException: String '+72 seconds UTC' contains non-relative elements
====

Warning: date_interval_create_from_date_string(): String 'next weekday 15:30' contains non-relative elements in %s on line %d

Warning: date_interval_create_from_date_string(): String '+5 hours noon' contains non-relative elements in %s on line %d

Warning: date_interval_create_from_date_string(): String '-8 days March 23' contains non-relative elements in %s on line %d

Warning: date_interval_create_from_date_string(): String '+72 seconds UTC' contains non-relative elements in %s on line %d"#,
    );
}

/// ext/date/tests/date_modify_basic1.phpt
#[test]
fn date_modify_basic1() {
    let mut datetime = DT::new("2009-01-31 14:28:41", "Europe/London");
    let mut out = String::from("*** Testing date_modify() : basic functionality ***\n");
    for (i, (m, f)) in [
        ("+1 day", "D, d M Y"),
        ("+1 week 2 days 4 hours 2 seconds", "D, d M Y H:i:s"),
        ("next Thursday", "D, d M Y"),
        ("last Sunday", "D, d M Y"),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(datetime.modify(m));
        let _ = writeln!(out, "After modification {}: {}", i + 1, datetime.format(f));
    }
    assert_text(
        &out,
        r#"*** Testing date_modify() : basic functionality ***
After modification 1: Sun, 01 Feb 2009
After modification 2: Tue, 10 Feb 2009 18:28:43
After modification 3: Thu, 12 Feb 2009
After modification 4: Sun, 08 Feb 2009"#,
    );
}

/// ext/date/tests/date_period.phpt (DatePeriod iteration via `date_period_advance()`)
#[test]
fn date_period() {
    let z = "UTC";
    let mut out = String::new();
    let db = DT::new("2008-01-01", z);
    let de = DT::new("2008-12-31", z);
    let di = php::date_interval_create_from_date_string("first day of next month").unwrap();
    for t in period_with_end(&db.t, &di.diff, &de.t, true, false) {
        let mut dt = DT {
            t,
            default_zone: z.into(),
        };
        assert!(dt.modify("3 tuesday"));
        out += &dt.format("l Y-m-d\n");
    }
    out += "\n";
    let db = DT::new("2007-12-31", z);
    let de = DT::new("2009-12-31 23:59:59", z);
    let di = php::date_interval_create_from_date_string("last thursday of next month").unwrap();
    for t in period_with_end(&db.t, &di.diff, &de.t, false, false) {
        let dt = DT {
            t,
            default_zone: z.into(),
        };
        out += &dt.format("l Y-m-d H:i:s\n");
    }
    assert_text(
        &out,
        r#"Tuesday 2008-01-15
Tuesday 2008-02-19
Tuesday 2008-03-18
Tuesday 2008-04-15
Tuesday 2008-05-20
Tuesday 2008-06-17
Tuesday 2008-07-15
Tuesday 2008-08-19
Tuesday 2008-09-16
Tuesday 2008-10-21
Tuesday 2008-11-18
Tuesday 2008-12-16

Thursday 2008-01-31 00:00:00
Thursday 2008-02-28 00:00:00
Thursday 2008-03-27 00:00:00
Thursday 2008-04-24 00:00:00
Thursday 2008-05-29 00:00:00
Thursday 2008-06-26 00:00:00
Thursday 2008-07-31 00:00:00
Thursday 2008-08-28 00:00:00
Thursday 2008-09-25 00:00:00
Thursday 2008-10-30 00:00:00
Thursday 2008-11-27 00:00:00
Thursday 2008-12-25 00:00:00
Thursday 2009-01-29 00:00:00
Thursday 2009-02-26 00:00:00
Thursday 2009-03-26 00:00:00
Thursday 2009-04-30 00:00:00
Thursday 2009-05-28 00:00:00
Thursday 2009-06-25 00:00:00
Thursday 2009-07-30 00:00:00
Thursday 2009-08-27 00:00:00
Thursday 2009-09-24 00:00:00
Thursday 2009-10-29 00:00:00
Thursday 2009-11-26 00:00:00
Thursday 2009-12-31 00:00:00"#,
    );
}

/// ext/date/tests/date_period_properties.phpt (partial: the DatePeriod properties that
/// `createFromISO8601String()` derives from the ISO string; the subclass property and
/// the json/serialize/var_export output are not ported)
#[test]
fn date_period_properties() {
    let r = php_timelib::strtointerval("R4/2012-07-01T00:00:00Z/P7D");
    assert!(r.errors.errors.is_empty());
    let mut start = r.begin.unwrap();
    start.update_ts(None);
    let start = DT {
        t: start,
        default_zone: "UTC".into(),
    };
    let end = r.end.map(|mut e| {
        e.update_ts(None);
        DT {
            t: e,
            default_zone: "UTC".into(),
        }
    });
    let interval = php::PhpInterval {
        diff: r.period.unwrap(),
        wall: true,
    };
    // recurrences + include_start_date
    let recurrences = i64::from(r.recurrences.unwrap_or(0)) + 1;
    let out = String::from("object(MyDatePeriod)#1 (8) {\n  [\"prop\"]=>\n  int(3)\n")
        + &period_props_dump(
            "DateTimeImmutable",
            &start,
            end.as_ref(),
            &interval,
            recurrences,
            true,
            false,
        )
        + "}\n";
    assert_expectf(
        &out,
        r#"object(MyDatePeriod)#%d (%d) {
  ["prop"]=>
  int(3)
  ["start"]=>
  object(DateTimeImmutable)#%d (%d) {
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
  object(DateInterval)#%d (%d) {
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
}"#,
    );
}

/// ext/date/tests/date_time_fractions_create_from_format.phpt
#[test]
fn date_time_fractions_create_from_format() {
    let dt1 = DT::from_format("Y-m-d H:i:s.u", "2016-10-03 12:47:18.819313", None, "UTC").unwrap();
    let dt2 = DT::from_format("U.u", "1475500799.176312", None, "UTC").unwrap();
    let out = dt1.var_dump("DateTime", 1) + &dt2.var_dump("DateTime", 2);
    assert_expectf(
        &out,
        r#"object(DateTime)#%d (%d) {
  ["date"]=>
  string(26) "2016-10-03 12:47:18.819313"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTime)#%d (%d) {
  ["date"]=>
  string(26) "2016-10-03 13:19:59.176312"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}"#,
    );
}

/// ext/date/tests/date_timestamp_get.phpt
#[test]
fn date_timestamp_get() {
    let begin_timestamp = DT::new("1970-01-01T00:00:00UTC", "UTC").timestamp();
    let date_time_tz = DT::new("1970-01-01T00:00:00UTC", "UTC").timestamp();
    let out = dump_int(begin_timestamp) + &dump_bool(date_time_tz == begin_timestamp);
    assert_text(
        &out,
        r#"int(0)
bool(true)"#,
    );
}

/// ext/date/tests/default-timezone-2.phpt
#[test]
fn default_timezone_2() {
    assert_eq!(
        strtotime_abs("2005-06-18 22:15:44", "Europe/Oslo"),
        Some(1119125744)
    );
}

/// ext/date/tests/getdate_variation3.phpt
#[test]
fn getdate_variation3() {
    let z = "Asia/Calcutta";
    let mut out = String::from("*** Testing getdate() : usage variation ***\n");
    for (key, value) in [
        ("hexadcimal 0x5", 0x5),
        ("hexadcimal 0xCAFE", 0xCAFE),
        ("octal -0xCAFE", -0xCAFE),
    ] {
        let _ = writeln!(out, "\n--{key}--");
        out += &getdate(value, z).var_dump();
    }
    assert_text(
        &out,
        r#"*** Testing getdate() : usage variation ***

--hexadcimal 0x5--
array(11) {
  ["seconds"]=>
  int(5)
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
  int(5)
}

--hexadcimal 0xCAFE--
array(11) {
  ["seconds"]=>
  int(6)
  ["minutes"]=>
  int(56)
  ["hours"]=>
  int(19)
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
  int(51966)
}

--octal -0xCAFE--
array(11) {
  ["seconds"]=>
  int(54)
  ["minutes"]=>
  int(3)
  ["hours"]=>
  int(15)
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
  int(-51966)
}"#,
    );
}

/// ext/date/tests/gh10218.phpt ("now" is the fixed `common::NOW`; the EXPECTF does not
/// check the date)
#[test]
fn gh10218() {
    let out = DT::new_in("now", "Etc/GMT+1", "UTC").var_dump("DateTime", 1);
    assert_expectf(
        &out,
        r#"object(DateTime)#%d (%d) {
  ["date"]=>
  string(%d) "%s"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(9) "Etc/GMT+1"
}"#,
    );
}

/// ext/date/tests/gh10747-4.phpt (partial: the DatePeriod's own properties, before and
/// after an emulated unserialize(); the subclass properties and the serialised string
/// are not ported)
#[test]
fn gh10747_4() {
    let start = DT::new("2023-03-03 16:24", "UTC");
    let end = DT::new("2023-03-09 16:24", "UTC");
    let interval = php::date_interval_create_from_date_string("+1 hour").unwrap();
    let expected = r#"  ["start"]=>
  object(DateTimeImmutable)#%d (%d) {
    ["date"]=>
    string(26) "2023-03-03 16:24:00.000000"
    ["timezone_type"]=>
    int(3)
    ["timezone"]=>
    string(3) "UTC"
  }
  ["current"]=>
  NULL
  ["end"]=>
  object(DateTimeImmutable)#%d (%d) {
    ["date"]=>
    string(26) "2023-03-09 16:24:00.000000"
    ["timezone_type"]=>
    int(3)
    ["timezone"]=>
    string(3) "UTC"
  }
  ["interval"]=>
  object(DateInterval)#%d (%d) {
    ["y"]=>
    int(0)
    ["m"]=>
    int(0)
    ["d"]=>
    int(0)
    ["h"]=>
    int(1)
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
  int(1)
  ["include_start_date"]=>
  bool(true)
  ["include_end_date"]=>
  bool(false)"#;
    let i = period_props_dump(
        "DateTimeImmutable",
        &start,
        Some(&end),
        &interval,
        1,
        true,
        false,
    );
    assert_expectf(&i, expected);
    let (start_u, end_u) = (
        date_roundtrip(&start).unwrap(),
        date_roundtrip(&end).unwrap(),
    );
    let u = period_props_dump(
        "DateTimeImmutable",
        &start_u,
        Some(&end_u),
        &interval,
        1,
        true,
        false,
    );
    assert_expectf(&u, expected);
}

/// ext/date/tests/gh20764.phpt (unserialize(serialize()) emulated with the object
/// properties and `php_date_initialize_from_hash()`)
#[test]
fn gh20764() {
    let mut out = String::new();
    for timezone in ["+03:00:30", "-03:00:30"] {
        let _ = writeln!(out, "--- Testing timezone {timezone} ---");
        let tz = php::timezone_open(timezone).unwrap();
        let dt = DT::try_new("2025-04-01", Some(&tz), "UTC").unwrap();
        out += &dump_str(&dt.format("e"));
        out += &dt.var_dump("DateTimeImmutable", 2);
        let u = date_roundtrip(&dt).unwrap();
        out += &tz_var_dump(&PhpTimeZone::from_time(&u.t).unwrap(), 3);
    }
    assert_expectf(
        &out,
        r#"--- Testing timezone +03:00:30 ---
string(9) "+03:00:30"
object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2025-04-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(9) "+03:00:30"
}
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(9) "+03:00:30"
}
--- Testing timezone -03:00:30 ---
string(9) "-03:00:30"
object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2025-04-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(9) "-03:00:30"
}
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(9) "-03:00:30"
}"#,
    );
}

/// ext/date/tests/gh9866.phpt
#[test]
fn gh9866() {
    let start = DT::new_in("2000-11-01 09:29:22.907606", "America/Chicago", "UTC");
    let end = DT::new_in("2022-06-06 11:00:00.000000", "America/New_York", "UTC");
    let years = php::date_diff(&start.t, &end.t, true).diff.y;
    let diff = start.diff(&end);
    let out = dump_int(years) + &interval_format(&diff, "%R %Y %M %D (%a) %H %I %S %F") + "\n";
    assert_text(
        &out,
        r#"int(21)
+ 21 07 04 (7886) 23 30 37 092394"#,
    );
}

/// ext/date/tests/gmdate_variation12.phpt (partial: a 32-bit only test; the in-range
/// timestamps are platform independent, the out-of-range warnings are not)
#[test]
fn gmdate_variation12() {
    let z = "UTC";
    assert_eq!(
        gmdate(DATE_ISO8601, mktime(20, 45, 54, 12, 13, 1901, z)),
        "1901-12-13T20:45:54+0000"
    );
    assert_eq!(
        gmdate(DATE_ISO8601, mktime(3, 14, 7, 1, 19, 2038, z)),
        "2038-01-19T03:14:07+0000"
    );
}

/// ext/date/tests/gmdate_variation6.phpt (partial: the calls with a timestamp)
#[test]
fn gmdate_variation6() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let mut out = String::new();
    for f in ["F", "m", "M", "n", "t"] {
        out += &dump_str(&gmdate(f, timestamp));
    }
    assert_text(
        &out,
        "string(6) \"August\"\nstring(2) \"08\"\nstring(3) \"Aug\"\nstring(1) \"8\"\nstring(2) \"31\"\n",
    );
}

/// ext/date/tests/mktime-1.phpt
#[test]
fn mktime_1() {
    let z = "Europe/Amsterdam";
    let mut out = String::new();
    for (m, d, y) in [
        (3, 0, 2000),
        (3, -1, 2000),
        (2, 29, 2000),
        (3, 0, 2001),
        (2, 29, 2001),
        (0, 0, 2000),
    ] {
        out += &date("Y-m-d", mktime(12, 0, 0, m, d, y, z), z);
        out += "\n";
    }
    assert_text(
        &out,
        r#"2000-02-29
2000-02-28
2000-02-29
2001-02-28
2001-03-01
1999-11-30"#,
    );
}

/// ext/date/tests/oo_001.phpt (partial: the parse error message and the DateTimeZone
/// lines; uninitialised-object errors are engine behaviour)
#[test]
fn oo_001() {
    let mut out = String::new();
    let s = "1am todax";
    match DT::try_new(s, None, "UTC") {
        Ok(_) => out += "no exception\n",
        Err(e) => {
            let m = &e.errors[0];
            let _ = writeln!(
                out,
                "DateMalformedStringException: Failed to parse time string ({s}) at position {} ({}): {}",
                m.position, m.character as char, m.message
            );
        }
    }
    out += &dump_str(&php::timezone_open("UTC").unwrap().name());
    match php::timezone_open("GottaFindThisOne") {
        Ok(_) => out += "no exception\n",
        Err(e) => {
            let _ = writeln!(
                out,
                "DateInvalidTimeZoneException: DateTimeZone::__construct(): {e}"
            );
        }
    }
    assert_text(
        &out,
        r#"DateMalformedStringException: Failed to parse time string (1am todax) at position 4 (t): The timezone could not be found in the database
string(3) "UTC"
DateInvalidTimeZoneException: DateTimeZone::__construct(): Unknown or bad timezone (GottaFindThisOne)"#,
    );
}

/// ext/date/tests/oom_date_parse.phpt
#[test]
fn oom_date_parse() {
    let input = "\x01".repeat(4 * 1024 * 1024);
    let result = date_parse(&input);
    let V::Arr(errors) = v_get(&result, &K::S("errors".into())) else {
        panic!("errors is not an array");
    };
    assert_eq!(
        dump_int(errors.len() as i64),
        r#"int(1000)"#.to_string() + "\n"
    );
}

/// ext/date/tests/oom_datetimeimmutable_construct.phpt
#[test]
fn oom_datetimeimmutable_construct() {
    let input = "\x01".repeat(4 * 1024 * 1024);
    let e = DT::try_new(&input, None, "UTC").expect_err("parse should fail");
    let m = &e.errors[0];
    assert_eq!((m.position, m.message), (0, "Unexpected character"));
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type3-bd1.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type3_bd1() {
    let z = "America/New_York";
    let date_format = "Y-m-d H:i:s T e";
    let interval_fmt = "P%dDT%hH";
    let mut out = String::new();
    for (label, e, s) in [
        ("bd1", "2010-11-07 05:30:00", "2010-11-06 04:30:00"),
        ("bd2", "2010-11-07 04:30:00", "2010-11-06 04:30:00"),
        ("bd3", "2010-11-07 03:30:00", "2010-11-06 04:30:00"),
        ("bd4", "2010-11-07 02:30:00", "2010-11-06 04:30:00"),
        ("bd7", "2010-11-07 01:30:00", "2010-11-06 01:30:00"),
    ] {
        let end = DT::new(e, z);
        let start = DT::new(s, z);
        let _ = writeln!(
            out,
            "{label} {} - {} = {}",
            end.format(date_format),
            start.format(date_format),
            interval_format(&start.diff(&end), interval_fmt)
        );
    }
    out += "\n";
    assert_text(
        &out,
        r#"bd1 2010-11-07 05:30:00 EST America/New_York - 2010-11-06 04:30:00 EDT America/New_York = P1DT1H
bd2 2010-11-07 04:30:00 EST America/New_York - 2010-11-06 04:30:00 EDT America/New_York = P1DT0H
bd3 2010-11-07 03:30:00 EST America/New_York - 2010-11-06 04:30:00 EDT America/New_York = P0DT24H
bd4 2010-11-07 02:30:00 EST America/New_York - 2010-11-06 04:30:00 EDT America/New_York = P0DT23H
bd7 2010-11-07 01:30:00 EDT America/New_York - 2010-11-06 01:30:00 EDT America/New_York = P0DT24H"#,
    );
}

/// ext/date/tests/strtotime3-64bit.phpt
#[test]
fn strtotime3_64bit() {
    let z = "Europe/Lisbon";
    let time = 1150494719;
    let strs = [
        "",
        " \t\r\n000",
        "yesterday",
        "22:49:12",
        "22:49:12 bogusTZ",
        "22.49.12.42GMT",
        "22.49.12.42bogusTZ",
        "t0222",
        "t0222 t0222",
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
        out += &match strtotime(s, time, z) {
            Some(t) => dump_str(&date(DATE_RFC2822, t, z)),
            None => dump_bool(false),
        };
    }
    assert_text(
        &out,
        r#"bool(false)
bool(false)
string(31) "Thu, 15 Jun 2006 00:00:00 +0100"
string(31) "Fri, 16 Jun 2006 22:49:12 +0100"
bool(false)
string(31) "Fri, 16 Jun 2006 23:49:12 +0100"
bool(false)
string(31) "Fri, 16 Jun 2006 02:22:00 +0100"
string(31) "Sun, 16 Jun 0222 02:22:00 -0036"
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
string(31) "Mon, 16 Jan 2006 00:00:00 +0000""#,
    );
}

/// ext/date/tests/sunfuncts_partial_hour_utc_offset.phpt
#[test]
fn sunfuncts_partial_hour_utc_offset() {
    use php_timelib::{SunFormat, SunResult, SunValue};
    let z = "Asia/Kolkata";
    let timestamp = mktime(0, 0, 0, 2, 9, 2025, z);
    let longitude = 22.57;
    let latitude = 88.36;
    // The test passes $longitude as the latitude argument and vice versa.
    let rise = php_timelib::sunrise_sunset(
        timestamp,
        SunFormat::String,
        longitude,
        latitude,
        90.833333,
        None,
        &tz(z),
        false,
    );
    let mut out = match rise {
        Some(SunResult::String(s)) => s,
        other => format!("{other:?}"),
    };
    out += "\n";
    let info = php_timelib::sun_info(timestamp, longitude, latitude, &tz(z));
    let SunValue::Time(sunrise) = info.sunrise else {
        panic!("no sunrise: {info:?}");
    };
    let mut dt = DT::new("now", z);
    dt.set_timestamp(sunrise);
    out += &dt.format("H:i");
    out += "\n";
    assert_text(
        &out,
        r#"06:10
06:11"#,
    );
}

/// ext/date/tests/timezone_identifiers_list_basic1.phpt
#[test]
fn timezone_identifiers_list_basic1() {
    // timezone_identifiers_list(DateTimeZone::ALL): check_id_allowed() and the BC flag.
    let prefixes = [
        "Africa/",
        "America/",
        "Antarctica/",
        "Arctic/",
        "Asia/",
        "Atlantic/",
        "Australia/",
        "Europe/",
        "Indian/",
        "Pacific/",
        "UTC",
    ];
    let db = php_timelib::builtin_db();
    let zones: Vec<&str> = db
        .identifiers()
        .iter()
        .filter(|e| {
            prefixes
                .iter()
                .any(|p| e.id.len() >= p.len() && e.id[..p.len()].eq_ignore_ascii_case(p))
                && db.entry_is_bc(e)
        })
        .map(|e| e.id.as_str())
        .collect();
    let mut out = String::from(
        "*** Testing timezone_identifiers_list() : basic functionality ***\nCheck return tpe is ARRAY\n",
    );
    out += &dump_bool(true);
    out += "Check array contains some common timezones\n";
    for z in ["Europe/London", "America/New_York", "UTC"] {
        out += &dump_bool(zones.contains(&z));
    }
    assert_text(
        &out,
        r#"*** Testing timezone_identifiers_list() : basic functionality ***
Check return tpe is ARRAY
bool(true)
Check array contains some common timezones
bool(true)
bool(true)
bool(true)"#,
    );
}

/// ext/date/tests/timezone_open_basic1.phpt
#[test]
fn timezone_open_basic1() {
    let mut out = String::from("*** Testing timezone_open() : basic functionality ***\n");
    for (i, z) in ["GMT", "Europe/London", "America/Los_Angeles"]
        .into_iter()
        .enumerate()
    {
        out += &tz_var_dump(&php::timezone_open(z).unwrap(), i as u32 + 1);
    }
    assert_expectf(
        &out,
        r#"*** Testing timezone_open() : basic functionality ***
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "GMT"
}
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(19) "America/Los_Angeles"
}"#,
    );
}

/// ext/date/tests/timezones.phpt (partial: the return values; the notices are not
/// ported)
#[test]
fn timezones() {
    let mut out = String::new();
    for z in ["AAA", "ZZZ", "Africa/Abidjan", "Zulu"] {
        out += &dump_bool(php_timelib::timezone_id_is_valid(z));
    }
    out += "done\n";
    assert_text(
        &out,
        "bool(false)\nbool(false)\nbool(true)\nbool(true)\ndone\n",
    );
}
