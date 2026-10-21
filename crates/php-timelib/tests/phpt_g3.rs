//! Tests ported from php-src `ext/date/tests/*.phpt` (group 3).
//!
//! Each test names its phpt file. Expected outputs are copied verbatim; for EXPECTF
//! sections only the deterministic parts are asserted (see [`assert_expectf`]), and
//! PHP warnings/deprecations (which this crate does not emit) are left out.
mod common;
use common::*;

use php_timelib::php::{self, PhpInterval, PhpTimeZone};
use php_timelib::{RelTime, SunFormat, SunResult, SunValue, Time, ZoneType};

const NY: &str = "America/New_York";

const DATE_ATOM: &str = "Y-m-d\\TH:i:sP";
const DATE_COOKIE: &str = "l, d-M-Y H:i:s T";
const DATE_ISO8601: &str = "Y-m-d\\TH:i:sO";
const DATE_ISO8601_EXPANDED: &str = "X-m-d\\TH:i:sP";
const DATE_RFC822: &str = "D, d M y H:i:s O";
const DATE_RFC850: &str = "l, d-M-y H:i:s T";
const DATE_RFC1036: &str = "D, d M y H:i:s O";
const DATE_RFC1123: &str = "D, d M Y H:i:s O";
const DATE_RFC7231: &str = "D, d M Y H:i:s \\G\\M\\T";
const DATE_RFC2822: &str = "D, d M Y H:i:s O";
const DATE_RFC3339: &str = "Y-m-d\\TH:i:sP";
const DATE_RFC3339_EXTENDED: &str = "Y-m-d\\TH:i:s.vP";
const DATE_RSS: &str = DATE_RFC1123;
const DATE_W3C: &str = DATE_RFC3339;

// ---------------------------------------------------------------------------------
// Local helpers

/// Matches `a` against a run-tests.php EXPECTF pattern. Supports `%s`, `%S`, `%d`,
/// `%i`, `%a`, `%A`, `%x`, `%w`, `%c` and `%%`.
fn expectf_match(a: &[u8], p: &[u8]) -> bool {
    let (mut ai, mut pi) = (0, 0);
    loop {
        if pi == p.len() {
            return ai == a.len();
        }
        if p[pi] == b'%' && pi + 1 < p.len() && p[pi + 1] != b'%' {
            let rest = &p[pi + 2..];
            let a = &a[ai..];
            let (min, ok): (usize, fn(u8) -> bool) = match p[pi + 1] {
                b's' => (1, |c| c != b'\n' && c != b'\r'),
                b'S' => (0, |c| c != b'\n' && c != b'\r'),
                b'd' => (1, |c| c.is_ascii_digit()),
                b'i' => {
                    let skip = usize::from(matches!(a.first(), Some(b'+' | b'-')));
                    let n = a[skip..].iter().take_while(|c| c.is_ascii_digit()).count();
                    return (1..=n).rev().any(|k| expectf_match(&a[skip + k..], rest));
                }
                b'a' => (1, |_| true),
                b'A' => (0, |_| true),
                b'x' => (1, |c| c.is_ascii_hexdigit()),
                b'w' => (0, |c| c.is_ascii_whitespace()),
                b'c' => return !a.is_empty() && expectf_match(&a[1..], rest),
                other => panic!("unsupported EXPECTF token %{}", other as char),
            };
            let n = a.iter().take_while(|&&c| ok(c)).count();
            return (min..=n).rev().any(|k| expectf_match(&a[k..], rest));
        }
        if p[pi] == b'%' {
            pi += 1;
        }
        if ai == a.len() || a[ai] != p[pi] {
            return false;
        }
        ai += 1;
        pi += 1;
    }
}

/// Asserts that `actual` matches the EXPECTF pattern `expected` (ends trimmed).
#[track_caller]
fn assert_expectf(actual: &str, expected: &str) {
    let a = actual.trim_end();
    let e = expected.trim_end();
    if !expectf_match(a.as_bytes(), e.as_bytes()) {
        let mut msg = String::new();
        for (i, (la, le)) in a.lines().zip(e.lines()).enumerate() {
            if !expectf_match(la.as_bytes(), le.as_bytes()) {
                msg = format!(
                    "first non-matching line {}:\n  actual:   {la:?}\n  expected: {le:?}\n",
                    i + 1
                );
                break;
            }
        }
        panic!("EXPECTF mismatch\n{msg}--- actual ---\n{a}\n--- expected ---\n{e}");
    }
}

/// `var_dump()` of a string.
fn vd_str(s: &str) -> String {
    V::Str(s.to_string()).var_dump()
}

/// `var_dump()` of a nullable/false-able result.
fn vd_bool_false() -> String {
    V::Bool(false).var_dump()
}

/// The properties of a `DateTimeZone` as `var_dump()` shows them.
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

/// `DateTime::getTimezone()`.
fn get_timezone(d: &DT) -> PhpTimeZone {
    PhpTimeZone::from_time(&d.t).expect("time has a zone")
}

/// `timezone_offset_get($tz, $datetime)` / `DateTimeZone::getOffset()`.
fn tz_offset_get(z: &PhpTimeZone, t: &Time) -> i64 {
    match z {
        PhpTimeZone::Id(tz) => i64::from(tz.get_time_zone_info(t.sse).offset),
        PhpTimeZone::Offset(o) => *o,
        PhpTimeZone::Abbr(a) => a.utc_offset + i64::from(a.dst) * 3600,
    }
}

/// `DateTimeImmutable::modify()`: a modified copy.
fn modified(d: &DT, s: &str) -> DT {
    let mut n = d.clone();
    assert!(n.modify(s), "modify({s:?}) failed");
    n
}

/// A `DateTime` wrapping a raw time.
fn dt_of(t: Time, zone: &str) -> DT {
    DT {
        t,
        default_zone: zone.to_string(),
    }
}

/// Iterates a `DatePeriod` exactly like `date_period_it_rewind()`,
/// `date_period_it_has_more()` and `date_period_it_move_forward()` (which advance with
/// `date_period_advance()`: the interval as relative time plus `timelib_update_ts`).
fn period(
    start: &Time,
    interval: &RelTime,
    end: Option<&Time>,
    recurrences: i64,
    include_start: bool,
    include_end: bool,
) -> Vec<Time> {
    let recurrences = recurrences + i64::from(include_start) + i64::from(include_end);
    let advance = |t: &mut Time| {
        t.have_relative = true;
        t.relative = *interval;
        t.sse_uptodate = false;
        t.update_ts(None);
        t.update_from_sse();
    };
    let mut cur = start.clone();
    if !include_start {
        advance(&mut cur);
    }
    let mut out = Vec::new();
    let mut index = 0;
    loop {
        let more = match end {
            Some(e) if cur.sse == e.sse => {
                if include_end {
                    cur.us <= e.us
                } else {
                    cur.us < e.us
                }
            }
            Some(e) => cur.sse < e.sse,
            None => index < recurrences,
        };
        if !more {
            break;
        }
        out.push(cur.clone());
        advance(&mut cur);
        index += 1;
    }
    out
}

/// `timezone_identifiers_list($what)` (`check_id_allowed` + the BC flag).
fn timezone_identifiers_list(what: u32) -> Vec<String> {
    const GROUPS: [(u32, &str); 11] = [
        (0x0001, "Africa/"),
        (0x0002, "America/"),
        (0x0004, "Antarctica/"),
        (0x0008, "Arctic/"),
        (0x0010, "Asia/"),
        (0x0020, "Atlantic/"),
        (0x0040, "Australia/"),
        (0x0080, "Europe/"),
        (0x0100, "Indian/"),
        (0x0200, "Pacific/"),
        (0x0400, "UTC"),
    ];
    let allowed = |id: &str| {
        GROUPS.iter().any(|(bit, prefix)| {
            what & bit != 0
                && id.len() >= prefix.len()
                && id[..prefix.len()].eq_ignore_ascii_case(prefix)
        })
    };
    let db = php_timelib::builtin_db();
    db.identifiers()
        .iter()
        .filter(|e| what == 0x0FFF || (allowed(&e.id) && db.entry_is_bc(e)))
        .map(|e| e.id.clone())
        .collect()
}

const TZ_ALL: u32 = 0x07FF;

/// `timezone_abbreviations_list()`: entries grouped by abbreviation.
fn timezone_abbreviations_list() -> Vec<(K, V)> {
    let mut out: Vec<(K, V)> = Vec::new();
    for e in php_timelib::timezone_abbreviations_list() {
        let el = V::Arr(vec![
            (K::S("dst".into()), V::Bool(e.dst)),
            (K::S("offset".into()), V::Int(i64::from(e.gmtoffset))),
            (
                K::S("timezone_id".into()),
                e.full_tz_name.map_or(V::Null, |id| V::Str(id.into())),
            ),
        ]);
        let key = K::S(e.name.into());
        if let Some((_, V::Arr(a))) = out.iter_mut().find(|(k, _)| *k == key) {
            let n = a.len() as i64;
            a.push((K::I(n), el));
        } else {
            out.push((key, V::Arr(vec![(K::I(0), el)])));
        }
    }
    out
}

/// Looks up a key of a PHP array value.
fn get<'a>(v: &'a V, key: &str) -> &'a V {
    match v {
        V::Arr(a) => a
            .iter()
            .find(|(k, _)| *k == K::S(key.into()))
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("no key {key}")),
        _ => panic!("not an array"),
    }
}

/// PHP's string conversion of a float with `precision` significant digits (`%.*G`),
/// for values in [1, 1e15).
fn php_float_prec(f: f64, precision: usize) -> String {
    let int_digits = (f.abs().log10().floor() as usize) + 1;
    let s = format!("{:.*}", precision.saturating_sub(int_digits), f);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Which part of `examine_diff()` (from `examine_diff.inc`) to show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Diff,
    Days,
    Add,
    Sub,
}

/// One `examine_diff()` call from a `DateTime_data-*.inc` file: (name, end, end is
/// converted to zone type 3, start, start is converted to zone type 3, spec).
type Case = (
    &'static str,
    &'static str,
    bool,
    &'static str,
    bool,
    &'static str,
);

/// `examine_diff($end, $start, $expect_spec, ...)` with `PHPT_DATETIME_SHOW = show`.
fn examine_diff(end: &DT, start: &DT, spec: &str, show: Show) -> String {
    let start_date = start.format("Y-m-d H:i:s T");
    let end_date = end.format("Y-m-d H:i:s T");
    let mut iv = php::date_interval_create(&format!("P{}", &spec[2..])).unwrap();
    if &spec[1..2] == "-" {
        iv.diff.invert = true;
    }
    match show {
        Show::Diff => {
            let r = start.diff(end);
            let rs = interval_format(&r, "P%R%yY%mM%dDT%hH%iM%sS");
            format!("DIFF: {end_date} - {start_date} = **{rs}**")
        }
        Show::Days => format!("DAYS: **{}**", interval_format(&start.diff(end), "%a")),
        Show::Add => {
            let mut s = start.clone();
            s.add_interval(&iv);
            format!(
                "ADD: {start_date} + {spec} = **{}**",
                s.format("Y-m-d H:i:s T")
            )
        }
        Show::Sub => {
            let mut e = end.clone();
            e.sub_interval(&iv);
            format!(
                "SUB: {end_date} - {spec} = **{}**",
                e.format("Y-m-d H:i:s T")
            )
        }
    }
}

/// Runs the `examine_diff()` calls of a data file in zone `America/New_York`.
fn run_cases(cases: &[Case], show: Show) -> String {
    let mut out = String::new();
    for &(name, end, end_tz, start, start_tz, spec) in cases {
        let mut e = DT::new(end, NY);
        if end_tz {
            e.set_timezone(NY);
        }
        let mut s = DT::new(start, NY);
        if start_tz {
            s.set_timezone(NY);
        }
        out.push_str(&format!("{name}: {}\n", examine_diff(&e, &s, spec, show)));
    }
    out
}

/// Cases of `DateTime_data-dates.inc`: (name, end, end -> zone type 3, start, start -> zone type 3, spec).
#[rustfmt::skip]
const DATES: &[Case] = &[
    ("test__7", "2009-01-14", false, "2009-01-07", false, "P+0Y0M7DT0H0M0S"),
    ("test_years_positive__7_by_0_day", "2007-02-07", false, "2000-02-07", false, "P+7Y0M0DT0H0M0S"),
    ("test_years_positive__7_by_1_day", "2007-02-08", false, "2000-02-07", false, "P+7Y0M1DT0H0M0S"),
    ("test_years_positive__6_shy_1_day", "2007-02-06", false, "2000-02-07", false, "P+6Y11M30DT0H0M0S"),
    ("test_years_positive__7_by_1_month", "2007-03-07", false, "2000-02-07", false, "P+7Y1M0DT0H0M0S"),
    ("test_years_positive__6_shy_1_month", "2007-01-07", false, "2000-02-07", false, "P+6Y11M0DT0H0M0S"),
    ("test_years_positive__7_by_1_month_split_newyear", "2007-01-07", false, "1999-12-07", false, "P+7Y1M0DT0H0M0S"),
    ("test_years_positive__6_shy_1_month_split_newyear", "2006-12-07", false, "2000-01-07", false, "P+6Y11M0DT0H0M0S"),
    ("test_negative__7", "2009-01-07", false, "2009-01-14", false, "P-0Y0M7DT0H0M0S"),
    ("test_years_negative__7_by_0_day", "2000-02-07", false, "2007-02-07", false, "P-7Y0M0DT0H0M0S"),
    ("test_years_negative__7_by_1_day", "2000-02-07", false, "2007-02-08", false, "P-7Y0M1DT0H0M0S"),
    ("test_years_negative__6_shy_1_day", "2000-02-07", false, "2007-02-06", false, "P-6Y11M28DT0H0M0S"),
    ("test_years_negative__7_by_1_month", "2000-02-07", false, "2007-03-07", false, "P-7Y1M0DT0H0M0S"),
    ("test_years_negative__6_shy_1_month", "2000-02-07", false, "2007-01-07", false, "P-6Y11M0DT0H0M0S"),
    ("test_years_negative__7_by_1_month_split_newyear", "1999-12-07", false, "2007-01-07", false, "P-7Y1M0DT0H0M0S"),
    ("test_years_negative__6_shy_1_month_split_newyear", "2000-01-07", false, "2006-12-07", false, "P-6Y11M0DT0H0M0S"),
];
/// Cases of `DateTime_data-fall-type3-type3.inc`: (name, end, end -> zone type 3, start, start -> zone type 3, spec).
#[rustfmt::skip]
const FALL_T3: &[Case] = &[
    ("test_time_fall_type3_prev_type3_prev", "2010-11-06 18:38:28", false, "2010-10-04 02:18:48", false, "P+0Y1M2DT16H19M40S"),
    ("test_time_fall_type3_prev_type3_dt", "2010-11-07 00:10:20", false, "2010-11-06 18:38:28", false, "P+0Y0M0DT5H31M52S"),
    ("test_time_fall_type3_prev_type3_redodt", "2010-11-07 01:12:33 EDT", true, "2010-11-06 18:38:28", false, "P+0Y0M0DT6H34M5S"),
    ("test_time_fall_type3_prev_type3_redost", "2010-11-07 01:14:44 EST", true, "2010-11-06 18:38:28", false, "P+0Y0M0DT7H36M16S"),
    ("test_time_fall_type3_prev_type3_st", "2010-11-07 03:16:55", false, "2010-11-06 18:38:28", false, "P+0Y0M0DT9H38M27S"),
    ("test_time_fall_type3_prev_type3_post", "2010-11-08 19:59:59", false, "2010-11-06 18:38:28", false, "P+0Y0M2DT1H21M31S"),
    ("test_time_fall_type3_dt_type3_prev", "2010-11-06 18:38:28", false, "2010-11-07 00:10:20", false, "P-0Y0M0DT5H31M52S"),
    ("test_time_fall_type3_dt_type3_dt", "2010-11-07 00:15:35", false, "2010-11-07 00:10:20", false, "P+0Y0M0DT0H5M15S"),
    ("test_time_fall_type3_dt_type3_redodt", "2010-11-07 01:12:33 EDT", true, "2010-11-07 00:10:20", false, "P+0Y0M0DT1H2M13S"),
    ("test_time_fall_type3_dt_type3_redost", "2010-11-07 01:14:44 EST", true, "2010-11-07 00:10:20", false, "P+0Y0M0DT2H4M24S"),
    ("test_time_fall_type3_dt_type3_st", "2010-11-07 03:16:55", false, "2010-11-07 00:10:20", false, "P+0Y0M0DT4H6M35S"),
    ("test_time_fall_type3_dt_type3_post", "2010-11-08 19:59:59", false, "2010-11-07 00:10:20", false, "P+0Y0M1DT20H49M39S"),
    ("test_time_fall_type3_redodt_type3_prev", "2010-11-06 18:38:28", false, "2010-11-07 01:12:33 EDT", true, "P-0Y0M0DT6H34M5S"),
    ("test_time_fall_type3_redodt_type3_dt", "2010-11-07 00:10:20", false, "2010-11-07 01:12:33 EDT", true, "P-0Y0M0DT1H2M13S"),
    ("test_time_fall_type3_redodt_type3_redodt", "2010-11-07 01:15:35 EDT", true, "2010-11-07 01:12:33 EDT", true, "P+0Y0M0DT0H3M2S"),
    ("test_time_fall_type3_redodt_type3_redost", "2010-11-07 01:14:44 EST", true, "2010-11-07 01:12:33 EDT", true, "P+0Y0M0DT1H2M11S"),
    ("test_time_fall_type3_redodt_type3_st", "2010-11-07 03:16:55", false, "2010-11-07 01:12:33 EDT", true, "P+0Y0M0DT3H4M22S"),
    ("test_time_fall_type3_redodt_type3_post", "2010-11-08 19:59:59", false, "2010-11-07 01:12:33 EDT", true, "P+0Y0M1DT19H47M26S"),
    ("test_time_fall_type3_redost_type3_prev", "2010-11-06 18:38:28", false, "2010-11-07 01:14:44 EST", true, "P-0Y0M0DT7H36M16S"),
    ("test_time_fall_type3_redost_type3_dt", "2010-11-07 00:10:20", false, "2010-11-07 01:14:44 EST", true, "P-0Y0M0DT2H4M24S"),
    ("test_time_fall_type3_redost_type3_redodt", "2010-11-07 01:12:33 EDT", true, "2010-11-07 01:14:44 EST", true, "P-0Y0M0DT1H2M11S"),
    ("test_time_fall_type3_redost_type3_redost", "2010-11-07 01:16:54 EST", true, "2010-11-07 01:14:44 EST", true, "P+0Y0M0DT0H2M10S"),
    ("test_time_fall_type3_redost_type3_st", "2010-11-07 03:16:55", false, "2010-11-07 01:14:44 EST", true, "P+0Y0M0DT2H2M11S"),
    ("test_time_fall_type3_redost_type3_post", "2010-11-08 19:59:59", false, "2010-11-07 01:14:44 EST", true, "P+0Y0M1DT18H45M15S"),
    ("test_time_fall_type3_st_type3_prev", "2010-11-06 18:38:28", false, "2010-11-07 03:16:55", false, "P-0Y0M0DT9H38M27S"),
    ("test_time_fall_type3_st_type3_dt", "2010-11-07 00:10:20", false, "2010-11-07 03:16:55", false, "P-0Y0M0DT4H6M35S"),
    ("test_time_fall_type3_st_type3_redodt", "2010-11-07 01:12:33 EDT", true, "2010-11-07 03:16:55", false, "P-0Y0M0DT3H4M22S"),
    ("test_time_fall_type3_st_type3_redost", "2010-11-07 01:14:44 EST", true, "2010-11-07 03:16:55", false, "P-0Y0M0DT2H2M11S"),
    ("test_time_fall_type3_st_type3_st", "2010-11-07 05:19:56", false, "2010-11-07 03:16:55", false, "P+0Y0M0DT2H3M1S"),
    ("test_time_fall_type3_st_type3_post", "2010-11-08 19:59:59", false, "2010-11-07 03:16:55", false, "P+0Y0M1DT16H43M4S"),
    ("test_time_fall_type3_post_type3_prev", "2010-11-06 18:38:28", false, "2010-11-08 19:59:59", false, "P-0Y0M2DT1H21M31S"),
    ("test_time_fall_type3_post_type3_dt", "2010-11-07 00:10:20", false, "2010-11-08 19:59:59", false, "P-0Y0M1DT20H49M39S"),
    ("test_time_fall_type3_post_type3_redodt", "2010-11-07 01:12:33 EDT", true, "2010-11-08 19:59:59", false, "P-0Y0M1DT19H47M26S"),
    ("test_time_fall_type3_post_type3_redost", "2010-11-07 01:14:44 EST", true, "2010-11-08 19:59:59", false, "P-0Y0M1DT18H45M15S"),
    ("test_time_fall_type3_post_type3_st", "2010-11-07 03:16:55", false, "2010-11-08 19:59:59", false, "P-0Y0M1DT16H43M4S"),
    ("test_time_fall_type3_post_type3_post", "2010-11-08 19:59:59", false, "2010-11-08 18:57:55", false, "P+0Y0M0DT1H2M4S"),
    ("test_time_fall_type3_dtsec_type3_stsec", "2010-11-07 01:00:00 EST", true, "2010-11-07 01:59:59 EDT", true, "P+0Y0M0DT0H0M1S"),
    ("test_time_fall_type3_stsec_type3_dtsec", "2010-11-07 01:59:59 EDT", true, "2010-11-07 01:00:00 EST", true, "P-0Y0M0DT0H0M1S"),
];
/// Cases of `DateTime_data-spring-type3-type3.inc`: (name, end, end -> zone type 3, start, start -> zone type 3, spec).
#[rustfmt::skip]
const SPRING_T3: &[Case] = &[
    ("test_time_spring_type3_prev_type3_prev", "2010-03-13 18:38:28", false, "2010-02-11 02:18:48", false, "P+0Y1M2DT16H19M40S"),
    ("test_time_spring_type3_prev_type3_st", "2010-03-14 00:10:20", false, "2010-03-13 18:38:28", false, "P+0Y0M0DT5H31M52S"),
    ("test_time_spring_type3_prev_type3_dt", "2010-03-14 03:16:55", false, "2010-03-13 18:38:28", false, "P+0Y0M0DT7H38M27S"),
    ("test_time_spring_type3_prev_type3_post", "2010-03-15 19:59:59", false, "2010-03-13 18:38:28", false, "P+0Y0M2DT1H21M31S"),
    ("test_time_spring_type3_st_type3_prev", "2010-03-13 18:38:28", false, "2010-03-14 00:10:20", false, "P-0Y0M0DT5H31M52S"),
    ("test_time_spring_type3_st_type3_st", "2010-03-14 00:15:35", false, "2010-03-14 00:10:20", false, "P+0Y0M0DT0H5M15S"),
    ("test_time_spring_type3_st_type3_dt", "2010-03-14 03:16:55", false, "2010-03-14 00:10:20", false, "P+0Y0M0DT2H6M35S"),
    ("test_time_spring_type3_st_type3_post", "2010-03-15 19:59:59", false, "2010-03-14 00:10:20", false, "P+0Y0M1DT18H49M39S"),
    ("test_time_spring_type3_dt_type3_prev", "2010-03-13 18:38:28", false, "2010-03-14 03:16:55", false, "P-0Y0M0DT7H38M27S"),
    ("test_time_spring_type3_dt_type3_st", "2010-03-14 00:10:20", false, "2010-03-14 03:16:55", false, "P-0Y0M0DT2H6M35S"),
    ("test_time_spring_type3_dt_type3_dt", "2010-03-14 05:19:56", false, "2010-03-14 03:16:55", false, "P+0Y0M0DT2H3M1S"),
    ("test_time_spring_type3_dt_type3_post", "2010-03-15 19:59:59", false, "2010-03-14 03:16:55", false, "P+0Y0M1DT16H43M4S"),
    ("test_time_spring_type3_post_type3_prev", "2010-03-13 18:38:28", false, "2010-03-15 19:59:59", false, "P-0Y0M2DT1H21M31S"),
    ("test_time_spring_type3_post_type3_st", "2010-03-14 00:10:20", false, "2010-03-15 19:59:59", false, "P-0Y0M1DT18H49M39S"),
    ("test_time_spring_type3_post_type3_dt", "2010-03-14 03:16:55", false, "2010-03-15 19:59:59", false, "P-0Y0M1DT16H43M4S"),
    ("test_time_spring_type3_post_type3_post", "2010-03-15 19:59:59", false, "2010-03-15 18:57:55", false, "P+0Y0M0DT1H2M4S"),
    ("test_time_spring_type3_stsec_type3_dtsec", "2010-03-14 03:00:00", false, "2010-03-14 01:59:59", false, "P+0Y0M0DT0H0M1S"),
    ("test_time_spring_type3_dtsec_type3_stsec", "2010-03-14 01:59:59", false, "2010-03-14 03:00:00", false, "P-0Y0M0DT0H0M1S"),
];

// ---------------------------------------------------------------------------------
// Tests

/// Self-check of the EXPECTF matcher used below.
#[test]
fn expectf_matcher_self_check() {
    assert!(expectf_match(b"int(-12)", b"int(-%d)"));
    assert!(!expectf_match(b"int(12)", b"int(-%d)"));
    assert!(expectf_match(b"a b\nc", b"%s b\n%s"));
    assert!(!expectf_match(b"a\nb", b"%s"));
    assert!(!expectf_match(b"10/11/2005x", b"10/11/%d"));
    assert!(expectf_match(b"x 100%", b"x %d%%"));
    assert!(expectf_match(b"+5 -6", b"%i %i"));
}

/// ext/date/tests/005.phpt (partial: the argument-count error is not expressible)
#[test]
fn test_005() {
    let t = mktime(0, 0, 0, 6, 27, 2006, "UTC");
    assert_eq!(idate('1', 1, "UTC"), None);
    assert_eq!(idate('0', NOW, "UTC"), None);
    assert_eq!(idate('B', t, "UTC"), Some(41));
    assert_eq!(idate('[', t, "UTC"), None);
    assert_eq!(idate('\'', NOW, "UTC"), None);
}

/// ext/date/tests/010.phpt
#[test]
fn test_010() {
    let list = timezone_abbreviations_list();
    let utc = &list
        .iter()
        .find(|(k, _)| *k == K::S("utc".into()))
        .unwrap()
        .1;
    let out = utc.var_dump() + "Done\n";
    assert_text(
        &out,
        r#"array(5) {
  [0]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(0)
    ["timezone_id"]=>
    string(13) "Etc/Universal"
  }
  [1]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(0)
    ["timezone_id"]=>
    string(7) "Etc/UTC"
  }
  [2]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(0)
    ["timezone_id"]=>
    string(8) "Etc/Zulu"
  }
  [3]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(0)
    ["timezone_id"]=>
    string(3) "UTC"
  }
  [4]=>
  array(3) {
    ["dst"]=>
    bool(false)
    ["offset"]=>
    int(0)
    ["timezone_id"]=>
    string(3) "UTC"
  }
}
Done"#,
    );
}

/// ext/date/tests/68062.phpt (partial: the TypeError is not expressible)
#[test]
fn test_68062() {
    let tz = php::timezone_open("Europe/London").unwrap();
    let dt = DT::new_in("2014-09-20", "Europe/London", "UTC");
    assert_eq!(tz_offset_get(&tz, &dt.t), 3600);
}

/// ext/date/tests/DateInterval_serialize-001.phpt (partial: the original object and the
/// use of the unserialised interval; unserialize() yields a civil interval)
#[test]
fn dateinterval_serialize_001() {
    let d = php::date_interval_create("P2Y4DT6H8M").unwrap();
    let e = PhpInterval {
        diff: d.diff,
        wall: false,
    };
    let mut out = String::from("Original object:\n");
    out.push_str(&interval_var_dump(&d, 1));
    out.push_str("\n\nUsed serialised interval:\n");
    let now = DT::new("2022-04-22 16:25:11 BST", "Europe/London");
    let mut a = now.clone();
    a.add_interval(&e);
    out.push_str(&a.var_dump("DateTimeImmutable", 4));
    let mut s = now.clone();
    s.sub_interval(&e);
    out.push_str(&s.var_dump("DateTimeImmutable", 4));
    assert_text(
        &out,
        r#"Original object:
object(DateInterval)#1 (10) {
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
}


Used serialised interval:
object(DateTimeImmutable)#4 (3) {
  ["date"]=>
  string(26) "2024-04-26 22:33:11.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "BST"
}
object(DateTimeImmutable)#4 (3) {
  ["date"]=>
  string(26) "2020-04-18 10:17:11.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "BST"
}"#,
    );
}

/// ext/date/tests/DatePeriod_IteratorAggregate.phpt (partial: the iteration of the ISO
/// period; iterator-object and subclass semantics are not expressible)
#[test]
fn dateperiod_iteratoraggregate() {
    let r = php_timelib::strtointerval("R2/2012-07-01T00:00:00Z/P7D");
    assert!(r.errors.errors.is_empty());
    let mut start = r.begin.unwrap();
    start.update_ts(None);
    let interval = r.period.unwrap();
    let recurrences = i64::from(r.recurrences.unwrap());
    let mut blocks = Vec::new();
    for _ in 0..6 {
        let mut b = String::new();
        for (i, t) in period(&start, &interval, None, recurrences, true, false)
            .into_iter()
            .enumerate()
        {
            b.push_str(&format!("{i}: {}\n", dt_of(t, "UTC").format("Y-m-d")));
        }
        blocks.push(b);
    }
    let out = blocks.join("\n");
    let expected = r#"0: 2012-07-01
1: 2012-07-08
2: 2012-07-15

0: 2012-07-01
1: 2012-07-08
2: 2012-07-15

0: 2012-07-01
1: 2012-07-08
2: 2012-07-15

0: 2012-07-01
1: 2012-07-08
2: 2012-07-15

0: 2012-07-01
1: 2012-07-08
2: 2012-07-15

0: 2012-07-01
1: 2012-07-08
2: 2012-07-15

0: 1
1: 2
2: 3"#;
    let expected = expected.split("\n\n0: 1").next().unwrap();
    assert_text(&out, expected);
}

/// ext/date/tests/DatePeriod_no_advance_on_valid.phpt
#[test]
fn dateperiod_no_advance_on_valid() {
    let start = DT::from_format("Y-m-d H:i:s", "2022-01-01 00:00:00", None, "UTC").unwrap();
    let end = DT::from_format("Y-m-d H:i:s", "2022-01-04 00:00:00", None, "UTC").unwrap();
    let interval = php::date_interval_create_from_date_string("1 day").unwrap();
    let mut out = String::new();
    let run = |out: &mut String, include_start: bool| {
        for t in period(
            &start.t,
            &interval.diff,
            Some(&end.t),
            0,
            include_start,
            false,
        ) {
            out.push_str(&dt_of(t, "UTC").format("Y-m-d\n"));
        }
    };
    run(&mut out, true);
    out.push_str("---------STEP 2\n");
    run(&mut out, true);
    out.push_str("---------STEP 3\n");
    run(&mut out, false);
    out.push_str("---------STEP 4\n");
    run(&mut out, false);
    assert_text(
        &out,
        r#"2022-01-01
2022-01-02
2022-01-03
---------STEP 2
2022-01-01
2022-01-02
2022-01-03
---------STEP 3
2022-01-02
2022-01-03
---------STEP 4
2022-01-02
2022-01-03"#,
    );
}

/// ext/date/tests/DatePeriod_serialize-004.phpt (partial: the iteration only)
#[test]
fn dateperiod_serialize_004() {
    let s = DT::new("1978-12-22 09:15:00 Europe/Amsterdam", "Europe/London");
    let i = php::date_interval_create_from_date_string("first monday of next month").unwrap();
    let mut out = String::new();
    for t in period(&s.t, &i.diff, None, 7, false, false) {
        out.push_str(&dt_of(t, "Europe/London").format(DATE_ISO8601));
        out.push('\n');
    }
    assert_text(
        &out,
        "1979-01-01T09:15:00+0100
1979-02-05T09:15:00+0100
1979-03-05T09:15:00+0100
1979-04-02T09:15:00+0200
1979-05-07T09:15:00+0200
1979-06-04T09:15:00+0200
1979-07-02T09:15:00+0200",
    );
}

/// ext/date/tests/DateTimeImmutable_createFromMutable-002.phpt (partial: the TypeError
/// is not expressible)
#[test]
fn datetimeimmutable_createfrommutable_002() {
    let i = DT::new("2014-03-02 16:24:08", "Europe/London");
    assert_expectf(
        &i.var_dump("MyDateTimeImmutable", 1),
        r#"object(MyDateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}"#,
    );
}

/// ext/date/tests/DateTimeZone_getTransitions_bug1.phpt
#[test]
fn datetimezone_gettransitions_bug1() {
    let mut out = String::new();
    for (tzid, from) in [
        ("Europe/London", 1_648_342_200_i64),
        ("America/Los_Angeles", 1_648_557_596),
        ("America/Chicago", 1_293_861_600),
        ("Europe/Paris", 1_645_095_600),
    ] {
        let to = from + (2.5 * 366.0 * 24.0 * 60.0 * 60.0) as i64;
        out.push_str(&format!("{tzid} from @{from}-@{to}:\n\n"));
        let z = tz(tzid);
        for t in php::timezone_transitions_get(&z, from, to) {
            out.push_str(&format!(
                "{:12} {} {:6} {} {}\n",
                t.ts,
                t.time(),
                t.offset,
                if t.isdst { "DST" } else { " x " },
                t.abbr
            ));
        }
        out.push('\n');
    }
    assert_text(
        &out,
        r#"Europe/London from @1648342200-@1727398200:

  1648342200 2022-03-27T00:50:00+00:00      0  x  GMT
  1648342800 2022-03-27T01:00:00+00:00   3600 DST BST
  1667091600 2022-10-30T01:00:00+00:00      0  x  GMT
  1679792400 2023-03-26T01:00:00+00:00   3600 DST BST
  1698541200 2023-10-29T01:00:00+00:00      0  x  GMT
  1711846800 2024-03-31T01:00:00+00:00   3600 DST BST

America/Los_Angeles from @1648557596-@1727613596:

  1648557596 2022-03-29T12:39:56+00:00 -25200 DST PDT
  1667725200 2022-11-06T09:00:00+00:00 -28800  x  PST
  1678615200 2023-03-12T10:00:00+00:00 -25200 DST PDT
  1699174800 2023-11-05T09:00:00+00:00 -28800  x  PST
  1710064800 2024-03-10T10:00:00+00:00 -25200 DST PDT

America/Chicago from @1293861600-@1372917600:

  1293861600 2011-01-01T06:00:00+00:00 -21600  x  CST
  1300003200 2011-03-13T08:00:00+00:00 -18000 DST CDT
  1320562800 2011-11-06T07:00:00+00:00 -21600  x  CST
  1331452800 2012-03-11T08:00:00+00:00 -18000 DST CDT
  1352012400 2012-11-04T07:00:00+00:00 -21600  x  CST
  1362902400 2013-03-10T08:00:00+00:00 -18000 DST CDT

Europe/Paris from @1645095600-@1724151600:

  1645095600 2022-02-17T11:00:00+00:00   3600  x  CET
  1648342800 2022-03-27T01:00:00+00:00   7200 DST CEST
  1667091600 2022-10-30T01:00:00+00:00   3600  x  CET
  1679792400 2023-03-26T01:00:00+00:00   7200 DST CEST
  1698541200 2023-10-29T01:00:00+00:00   3600  x  CET
  1711846800 2024-03-31T01:00:00+00:00   7200 DST CEST"#,
    );
}

/// ext/date/tests/DateTimeZone_serialize_type_1.phpt (partial: serialisation skipped)
#[test]
fn datetimezone_serialize_type_1() {
    let tz1 = get_timezone(&DT::new("2012-01-01 10:00 +1:00", "Europe/London"));
    assert_expectf(
        &tz_var_dump(&tz1, 1),
        r#"object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}"#,
    );
    assert_eq!(tz1.name(), "+01:00");
}

/// ext/date/tests/DateTime_add-dates.phpt
#[test]
fn datetime_add_dates() {
    assert_text(
        &run_cases(DATES, Show::Add),
        r#"test__7: ADD: 2009-01-07 00:00:00 EST + P+0Y0M7DT0H0M0S = **2009-01-14 00:00:00 EST**
test_years_positive__7_by_0_day: ADD: 2000-02-07 00:00:00 EST + P+7Y0M0DT0H0M0S = **2007-02-07 00:00:00 EST**
test_years_positive__7_by_1_day: ADD: 2000-02-07 00:00:00 EST + P+7Y0M1DT0H0M0S = **2007-02-08 00:00:00 EST**
test_years_positive__6_shy_1_day: ADD: 2000-02-07 00:00:00 EST + P+6Y11M30DT0H0M0S = **2007-02-06 00:00:00 EST**
test_years_positive__7_by_1_month: ADD: 2000-02-07 00:00:00 EST + P+7Y1M0DT0H0M0S = **2007-03-07 00:00:00 EST**
test_years_positive__6_shy_1_month: ADD: 2000-02-07 00:00:00 EST + P+6Y11M0DT0H0M0S = **2007-01-07 00:00:00 EST**
test_years_positive__7_by_1_month_split_newyear: ADD: 1999-12-07 00:00:00 EST + P+7Y1M0DT0H0M0S = **2007-01-07 00:00:00 EST**
test_years_positive__6_shy_1_month_split_newyear: ADD: 2000-01-07 00:00:00 EST + P+6Y11M0DT0H0M0S = **2006-12-07 00:00:00 EST**
test_negative__7: ADD: 2009-01-14 00:00:00 EST + P-0Y0M7DT0H0M0S = **2009-01-07 00:00:00 EST**
test_years_negative__7_by_0_day: ADD: 2007-02-07 00:00:00 EST + P-7Y0M0DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_negative__7_by_1_day: ADD: 2007-02-08 00:00:00 EST + P-7Y0M1DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_negative__6_shy_1_day: ADD: 2007-02-06 00:00:00 EST + P-6Y11M28DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_negative__7_by_1_month: ADD: 2007-03-07 00:00:00 EST + P-7Y1M0DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_negative__6_shy_1_month: ADD: 2007-01-07 00:00:00 EST + P-6Y11M0DT0H0M0S = **2000-02-07 00:00:00 EST**
test_years_negative__7_by_1_month_split_newyear: ADD: 2007-01-07 00:00:00 EST + P-7Y1M0DT0H0M0S = **1999-12-07 00:00:00 EST**
test_years_negative__6_shy_1_month_split_newyear: ADD: 2006-12-07 00:00:00 EST + P-6Y11M0DT0H0M0S = **2000-01-07 00:00:00 EST**"#,
    );
}

/// ext/date/tests/DateTime_add-massive.phpt
#[test]
fn datetime_add_massive() {
    let make = |y: i64| {
        let mut d = DT::new("now", NY);
        d.set_date(y, 1, 1);
        d.set_time(16, 18, 2, 0);
        d
    };
    let out = format!(
        "test_massive_positive: {}\ntest_massive_negative: {}\n",
        examine_diff(
            &make(333_333),
            &make(-333_333),
            "P+666666Y0M0DT0H0M0S",
            Show::Add
        ),
        examine_diff(
            &make(-333_333),
            &make(333_333),
            "P-666666Y0M0DT0H0M0S",
            Show::Add
        ),
    );
    assert_expectf(
        &out,
        r#"test_massive_positive: ADD: -333333-01-01 16:18:02 %s + P+666666Y0M0DT0H0M0S = **333333-01-01 16:18:02 %s**
test_massive_negative: ADD: 333333-01-01 16:18:02 %s + P-666666Y0M0DT0H0M0S = **-333333-01-01 16:18:02 %s**"#,
    );
}

/// ext/date/tests/DateTime_construct-dst-overlap.phpt
#[test]
fn datetime_construct_dst_overlap() {
    let mut out = String::new();
    for (zone, s) in [
        (NY, "2011-11-06 01:30:00"),
        ("Atlantic/Azores", "2011-10-30 00:30:00"),
        ("Europe/London", "2011-10-30 01:30:00"),
        ("Europe/Amsterdam", "2011-10-30 02:30:00"),
    ] {
        out.push_str(&DT::new(s, zone).format("P"));
        out.push('\n');
    }
    assert_text(
        &out,
        r#"-04:00
+00:00
+00:00
+01:00"#,
    );
}

/// ext/date/tests/DateTime_createFromInterface-002.phpt
#[test]
fn datetime_createfrominterface_002() {
    let mut out = String::new();
    for current in [
        "2014-03-02 16:24:08",
        "2019-12-16 15:06:46 CET",
        "2019-12-16 15:08:20 +0100",
    ] {
        // createFromInterface() copies the time of a DateTime and a DateTimeImmutable.
        for _ in 0..2 {
            out.push_str(&DT::new(current, "Europe/London").var_dump("MyDateTime", 1));
        }
    }
    assert_expectf(
        &out,
        r#"object(MyDateTime)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(MyDateTime)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(MyDateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(MyDateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(MyDateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}
object(MyDateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}"#,
    );
}

/// ext/date/tests/DateTime_days-fall-type3-type3.phpt
#[test]
fn datetime_days_fall_type3_type3() {
    assert_text(
        &run_cases(FALL_T3, Show::Days),
        r#"test_time_fall_type3_prev_type3_prev: DAYS: **33**
test_time_fall_type3_prev_type3_dt: DAYS: **0**
test_time_fall_type3_prev_type3_redodt: DAYS: **0**
test_time_fall_type3_prev_type3_redost: DAYS: **0**
test_time_fall_type3_prev_type3_st: DAYS: **0**
test_time_fall_type3_prev_type3_post: DAYS: **2**
test_time_fall_type3_dt_type3_prev: DAYS: **0**
test_time_fall_type3_dt_type3_dt: DAYS: **0**
test_time_fall_type3_dt_type3_redodt: DAYS: **0**
test_time_fall_type3_dt_type3_redost: DAYS: **0**
test_time_fall_type3_dt_type3_st: DAYS: **0**
test_time_fall_type3_dt_type3_post: DAYS: **1**
test_time_fall_type3_redodt_type3_prev: DAYS: **0**
test_time_fall_type3_redodt_type3_dt: DAYS: **0**
test_time_fall_type3_redodt_type3_redodt: DAYS: **0**
test_time_fall_type3_redodt_type3_redost: DAYS: **0**
test_time_fall_type3_redodt_type3_st: DAYS: **0**
test_time_fall_type3_redodt_type3_post: DAYS: **1**
test_time_fall_type3_redost_type3_prev: DAYS: **0**
test_time_fall_type3_redost_type3_dt: DAYS: **0**
test_time_fall_type3_redost_type3_redodt: DAYS: **0**
test_time_fall_type3_redost_type3_redost: DAYS: **0**
test_time_fall_type3_redost_type3_st: DAYS: **0**
test_time_fall_type3_redost_type3_post: DAYS: **1**
test_time_fall_type3_st_type3_prev: DAYS: **0**
test_time_fall_type3_st_type3_dt: DAYS: **0**
test_time_fall_type3_st_type3_redodt: DAYS: **0**
test_time_fall_type3_st_type3_redost: DAYS: **0**
test_time_fall_type3_st_type3_st: DAYS: **0**
test_time_fall_type3_st_type3_post: DAYS: **1**
test_time_fall_type3_post_type3_prev: DAYS: **2**
test_time_fall_type3_post_type3_dt: DAYS: **1**
test_time_fall_type3_post_type3_redodt: DAYS: **1**
test_time_fall_type3_post_type3_redost: DAYS: **1**
test_time_fall_type3_post_type3_st: DAYS: **1**
test_time_fall_type3_post_type3_post: DAYS: **0**
test_time_fall_type3_dtsec_type3_stsec: DAYS: **0**
test_time_fall_type3_stsec_type3_dtsec: DAYS: **0**"#,
    );
}

/// ext/date/tests/DateTime_days-spring-type3-type3.phpt
#[test]
fn datetime_days_spring_type3_type3() {
    assert_text(
        &run_cases(SPRING_T3, Show::Days),
        r#"test_time_spring_type3_prev_type3_prev: DAYS: **30**
test_time_spring_type3_prev_type3_st: DAYS: **0**
test_time_spring_type3_prev_type3_dt: DAYS: **0**
test_time_spring_type3_prev_type3_post: DAYS: **2**
test_time_spring_type3_st_type3_prev: DAYS: **0**
test_time_spring_type3_st_type3_st: DAYS: **0**
test_time_spring_type3_st_type3_dt: DAYS: **0**
test_time_spring_type3_st_type3_post: DAYS: **1**
test_time_spring_type3_dt_type3_prev: DAYS: **0**
test_time_spring_type3_dt_type3_st: DAYS: **0**
test_time_spring_type3_dt_type3_dt: DAYS: **0**
test_time_spring_type3_dt_type3_post: DAYS: **1**
test_time_spring_type3_post_type3_prev: DAYS: **2**
test_time_spring_type3_post_type3_st: DAYS: **1**
test_time_spring_type3_post_type3_dt: DAYS: **1**
test_time_spring_type3_post_type3_post: DAYS: **0**
test_time_spring_type3_stsec_type3_dtsec: DAYS: **0**
test_time_spring_type3_dtsec_type3_stsec: DAYS: **0**"#,
    );
}

/// ext/date/tests/DateTime_diff-fall-type3-type3.phpt
#[test]
fn datetime_diff_fall_type3_type3() {
    assert_text(
        &run_cases(FALL_T3, Show::Diff),
        r#"test_time_fall_type3_prev_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-10-04 02:18:48 EDT = **P+0Y1M2DT16H19M40S**
test_time_fall_type3_prev_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT5H31M52S**
test_time_fall_type3_prev_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT6H34M5S**
test_time_fall_type3_prev_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT7H36M16S**
test_time_fall_type3_prev_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT9H38M27S**
test_time_fall_type3_prev_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M2DT1H21M31S**
test_time_fall_type3_dt_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 00:10:20 EDT = **P-0Y0M0DT5H31M52S**
test_time_fall_type3_dt_type3_dt: DIFF: 2010-11-07 00:15:35 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT0H5M15S**
test_time_fall_type3_dt_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT1H2M13S**
test_time_fall_type3_dt_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT2H4M24S**
test_time_fall_type3_dt_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT4H6M35S**
test_time_fall_type3_dt_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M1DT19H49M39S**
test_time_fall_type3_redodt_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT6H34M5S**
test_time_fall_type3_redodt_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT1H2M13S**
test_time_fall_type3_redodt_type3_redodt: DIFF: 2010-11-07 01:15:35 EDT - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT0H3M2S**
test_time_fall_type3_redodt_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT1H2M11S**
test_time_fall_type3_redodt_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT3H4M22S**
test_time_fall_type3_redodt_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M1DT18H47M26S**
test_time_fall_type3_redost_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT7H36M16S**
test_time_fall_type3_redost_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT2H4M24S**
test_time_fall_type3_redost_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT1H2M11S**
test_time_fall_type3_redost_type3_redost: DIFF: 2010-11-07 01:16:54 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT0H2M10S**
test_time_fall_type3_redost_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT2H2M11S**
test_time_fall_type3_redost_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:14:44 EST = **P+0Y0M1DT18H45M15S**
test_time_fall_type3_st_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT9H38M27S**
test_time_fall_type3_st_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT4H6M35S**
test_time_fall_type3_st_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT3H4M22S**
test_time_fall_type3_st_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 03:16:55 EST = **P-0Y0M0DT2H2M11S**
test_time_fall_type3_st_type3_st: DIFF: 2010-11-07 05:19:56 EST - 2010-11-07 03:16:55 EST = **P+0Y0M0DT2H3M1S**
test_time_fall_type3_st_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 03:16:55 EST = **P+0Y0M1DT16H43M4S**
test_time_fall_type3_post_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M2DT1H21M31S**
test_time_fall_type3_post_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT19H49M39S**
test_time_fall_type3_post_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT18H47M26S**
test_time_fall_type3_post_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT18H45M15S**
test_time_fall_type3_post_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT16H43M4S**
test_time_fall_type3_post_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-08 18:57:55 EST = **P+0Y0M0DT1H2M4S**
test_time_fall_type3_dtsec_type3_stsec: DIFF: 2010-11-07 01:00:00 EST - 2010-11-07 01:59:59 EDT = **P+0Y0M0DT0H0M1S**
test_time_fall_type3_stsec_type3_dtsec: DIFF: 2010-11-07 01:59:59 EDT - 2010-11-07 01:00:00 EST = **P-0Y0M0DT0H0M1S**"#,
    );
}

/// ext/date/tests/DateTime_diff-spring-type3-type3.phpt
#[test]
fn datetime_diff_spring_type3_type3() {
    assert_text(
        &run_cases(SPRING_T3, Show::Diff),
        r#"test_time_spring_type3_prev_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-02-11 02:18:48 EST = **P+0Y1M2DT16H19M40S**
test_time_spring_type3_prev_type3_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-13 18:38:28 EST = **P+0Y0M0DT5H31M52S**
test_time_spring_type3_prev_type3_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M0DT7H38M27S**
test_time_spring_type3_prev_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M2DT1H21M31S**
test_time_spring_type3_st_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 00:10:20 EST = **P-0Y0M0DT5H31M52S**
test_time_spring_type3_st_type3_st: DIFF: 2010-03-14 00:15:35 EST - 2010-03-14 00:10:20 EST = **P+0Y0M0DT0H5M15S**
test_time_spring_type3_st_type3_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M0DT2H6M35S**
test_time_spring_type3_st_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M1DT19H49M39S**
test_time_spring_type3_dt_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT7H38M27S**
test_time_spring_type3_dt_type3_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT2H6M35S**
test_time_spring_type3_dt_type3_dt: DIFF: 2010-03-14 05:19:56 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M0DT2H3M1S**
test_time_spring_type3_dt_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M1DT16H43M4S**
test_time_spring_type3_post_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M2DT1H21M31S**
test_time_spring_type3_post_type3_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT19H49M39S**
test_time_spring_type3_post_type3_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT16H43M4S**
test_time_spring_type3_post_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-15 18:57:55 EDT = **P+0Y0M0DT1H2M4S**
test_time_spring_type3_stsec_type3_dtsec: DIFF: 2010-03-14 03:00:00 EDT - 2010-03-14 01:59:59 EST = **P+0Y0M0DT0H0M1S**
test_time_spring_type3_dtsec_type3_stsec: DIFF: 2010-03-14 01:59:59 EST - 2010-03-14 03:00:00 EDT = **P-0Y0M0DT0H0M1S**"#,
    );
}

/// ext/date/tests/DateTime_sub-fall-type3-type3.phpt
#[test]
fn datetime_sub_fall_type3_type3() {
    assert_text(
        &run_cases(FALL_T3, Show::Sub),
        r#"test_time_fall_type3_prev_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P+0Y1M2DT16H19M40S = **2010-10-04 02:18:48 EDT**
test_time_fall_type3_prev_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P+0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type3_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT7H36M16S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT9H38M27S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_dt_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type3_dt: SUB: 2010-11-07 00:15:35 EDT - P+0Y0M0DT0H5M15S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type3_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT2H4M24S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT4H6M35S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT20H49M39S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_redodt_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type3_redodt: SUB: 2010-11-07 01:15:35 EDT - P+0Y0M0DT0H3M2S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type3_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT1H2M11S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT3H4M22S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT19H47M26S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redost_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT7H36M16S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redost_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT2H4M24S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redost_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT1H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redost_type3_redost: SUB: 2010-11-07 01:16:54 EST - P+0Y0M0DT0H2M10S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redost_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redost_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_st_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT9H38M27S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_st_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT4H6M35S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_st_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT3H4M22S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_st_type3_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_st_type3_st: SUB: 2010-11-07 05:19:56 EST - P+0Y0M0DT2H3M1S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_st_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_post_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_post_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EST**
test_time_fall_type3_post_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EST**
test_time_fall_type3_post_type3_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_post_type3_st: SUB: 2010-11-07 03:16:55 EST - P-0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_post_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M0DT1H2M4S = **2010-11-08 18:57:55 EST**
test_time_fall_type3_dtsec_type3_stsec: SUB: 2010-11-07 01:00:00 EST - P+0Y0M0DT0H0M1S = **2010-11-07 01:59:59 EDT**
test_time_fall_type3_stsec_type3_dtsec: SUB: 2010-11-07 01:59:59 EDT - P-0Y0M0DT0H0M1S = **2010-11-07 01:00:00 EST**"#,
    );
}

/// ext/date/tests/DateTime_sub-spring-type3-type3.phpt
#[test]
fn datetime_sub_spring_type3_type3() {
    assert_text(
        &run_cases(SPRING_T3, Show::Sub),
        r#"test_time_spring_type3_prev_type3_prev: SUB: 2010-03-13 18:38:28 EST - P+0Y1M2DT16H19M40S = **2010-02-11 02:18:48 EST**
test_time_spring_type3_prev_type3_st: SUB: 2010-03-14 00:10:20 EST - P+0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_prev_type3_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT7H38M27S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_prev_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_st_type3_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_st_type3_st: SUB: 2010-03-14 00:15:35 EST - P+0Y0M0DT0H5M15S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_st_type3_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT2H6M35S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_st_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT18H49M39S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_dt_type3_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT7H38M27S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_dt_type3_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M0DT2H6M35S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_dt_type3_dt: SUB: 2010-03-14 05:19:56 EDT - P+0Y0M0DT2H3M1S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_dt_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_post_type3_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_post_type3_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EDT**
test_time_spring_type3_post_type3_dt: SUB: 2010-03-14 03:16:55 EDT - P-0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_post_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M0DT1H2M4S = **2010-03-15 18:57:55 EDT**
test_time_spring_type3_stsec_type3_dtsec: SUB: 2010-03-14 03:00:00 EDT - P+0Y0M0DT0H0M1S = **2010-03-14 01:59:59 EST**
test_time_spring_type3_dtsec_type3_stsec: SUB: 2010-03-14 01:59:59 EST - P-0Y0M0DT0H0M1S = **2010-03-14 03:00:00 EDT**"#,
    );
}

/// ext/date/tests/DateTime_format_basic2.phpt
#[test]
fn datetime_format_basic2() {
    let date = DT::new("2005-07-14 22:30:41", "Europe/London");
    let mut out = String::from(
        "*** Testing date_format() : basic functionality - formatting constants ***\n",
    );
    for f in [
        DATE_ATOM,
        DATE_COOKIE,
        DATE_ISO8601,
        DATE_ISO8601_EXPANDED,
        DATE_RFC822,
        DATE_RFC850,
        DATE_RFC1036,
        DATE_RFC1123,
        DATE_RFC7231,
        DATE_RFC2822,
        DATE_RFC3339,
        DATE_RFC3339_EXTENDED,
        DATE_RSS,
        DATE_W3C,
    ] {
        out.push_str(&vd_str(&date.format(f)));
    }
    assert_text(
        &out,
        r#"*** Testing date_format() : basic functionality - formatting constants ***
string(25) "2005-07-14T22:30:41+01:00"
string(34) "Thursday, 14-Jul-2005 22:30:41 BST"
string(24) "2005-07-14T22:30:41+0100"
string(26) "+2005-07-14T22:30:41+01:00"
string(29) "Thu, 14 Jul 05 22:30:41 +0100"
string(32) "Thursday, 14-Jul-05 22:30:41 BST"
string(29) "Thu, 14 Jul 05 22:30:41 +0100"
string(31) "Thu, 14 Jul 2005 22:30:41 +0100"
string(29) "Thu, 14 Jul 2005 22:30:41 GMT"
string(31) "Thu, 14 Jul 2005 22:30:41 +0100"
string(25) "2005-07-14T22:30:41+01:00"
string(29) "2005-07-14T22:30:41.000+01:00"
string(31) "Thu, 14 Jul 2005 22:30:41 +0100"
string(25) "2005-07-14T22:30:41+01:00""#,
    );
}

/// ext/date/tests/bug-gh18076.phpt
#[test]
fn bug_gh18076() {
    let ts = strtotime_abs("2025-03-21", "UTC").unwrap();
    let si = php_timelib::sun_info(ts, 51.48, 0.0, &tz("UTC"));
    let (SunValue::Time(rise), SunValue::Time(set)) = (si.sunrise, si.sunset) else {
        panic!("no sunrise/sunset");
    };
    assert_eq!(date("H:i:s", rise, "UTC"), "05:59:21");
    assert_eq!(date("H:i:s", set, "UTC"), "18:14:48");
}

/// ext/date/tests/bug-gh9763.phpt
#[test]
fn bug_gh9763() {
    let mut out = String::new();
    for test in [
        "+99:60", "+99:62", "-99:62", "-99:60", "+9960", "-9960", "+9959", "-9959",
    ] {
        out.push_str(&format!("Testing {test}: "));
        match php::timezone_open(test) {
            Ok(z) => out.push_str(&format!("{}\n", z.name())),
            Err(m) => out.push_str(&format!(
                "DateInvalidTimeZoneException: DateTimeZone::__construct(): {m}\n"
            )),
        }
    }
    assert_text(
        &out,
        r#"Testing +99:60: DateInvalidTimeZoneException: DateTimeZone::__construct(): Timezone offset is out of range (+99:60)
Testing +99:62: DateInvalidTimeZoneException: DateTimeZone::__construct(): Timezone offset is out of range (+99:62)
Testing -99:62: DateInvalidTimeZoneException: DateTimeZone::__construct(): Timezone offset is out of range (-99:62)
Testing -99:60: DateInvalidTimeZoneException: DateTimeZone::__construct(): Timezone offset is out of range (-99:60)
Testing +9960: DateInvalidTimeZoneException: DateTimeZone::__construct(): Timezone offset is out of range (+9960)
Testing -9960: DateInvalidTimeZoneException: DateTimeZone::__construct(): Timezone offset is out of range (-9960)
Testing +9959: +99:59
Testing -9959: -99:59"#,
    );
}

/// ext/date/tests/bug21399.phpt
#[test]
fn bug21399() {
    let ts = strtotime_abs("20050620091407 GMT", "GMT").unwrap();
    assert_eq!(gmdate("Y-m-d H:i:s", ts), "2005-06-20 09:14:07");
}

/// ext/date/tests/bug26694.phpt
#[test]
fn bug26694() {
    let ts = strtotime_abs("Sun, 21 Dec 2003 20:38:33 +0000 GMT", "GMT").unwrap();
    assert_eq!(gmdate("Y-m-d H:i:s", ts), "2003-12-21 20:38:33");
}

/// ext/date/tests/bug29585.phpt
#[test]
fn bug29585() {
    let ts = strtotime_abs("2004W30", "GMT").unwrap();
    assert_eq!(gmdate("Y-m-d H:i:s", ts), "2004-07-19 00:00:00");
}

/// ext/date/tests/bug32555.phpt
#[test]
fn bug32555() {
    let z = "US/Eastern";
    let stamp = 1_112_427_000;
    let mut out = String::new();
    for s in ["now", "tomorrow", "+1 day", "+2 day"] {
        out.push_str(&date("r", strtotime(s, stamp, z).unwrap(), z));
        out.push('\n');
    }
    assert_text(
        &out,
        r#"Sat, 02 Apr 2005 02:30:00 -0500
Sun, 03 Apr 2005 00:00:00 -0500
Sun, 03 Apr 2005 03:30:00 -0400
Mon, 04 Apr 2005 02:30:00 -0400"#,
    );
}

/// `bug33415-2.phpt` cases: (zone, mktime() arguments, relative string, expected
/// "tStamp=" and "result=" lines).
#[rustfmt::skip]
const BUG33415_2: &[(&str, [i64; 6], &str, &str, &str)] = &[
    ("Africa/Bujumbura", [17, 17, 17, 1, 1, 1970], "next Wednesday", "Thursday 1970-01-01 17:17:17 CAT 0", "Wednesday 1970-01-07 00:00:00 CAT 0"),
    ("Asia/Thimbu", [17, 17, 17, 1, 6476, 1970], "next Thursday", "Thursday 1987-09-24 17:17:17 +0530 0", "Thursday 1987-10-01 00:30:00 +06 0"),
    ("Indian/Cocos", [17, 17, 17, 1, 1, 1970], "next Thursday", "Thursday 1970-01-01 17:17:17 +0630 0", "Thursday 1970-01-08 00:00:00 +0630 0"),
    ("Africa/Lubumbashi", [17, 17, 17, 1, 1, 1970], "next Saturday", "Thursday 1970-01-01 17:17:17 CAT 0", "Saturday 1970-01-03 00:00:00 CAT 0"),
    ("Asia/Kashgar", [17, 17, 17, 1, 3767, 1970], "next Thursday", "Thursday 1980-04-24 17:17:17 +06 0", "Thursday 1980-05-01 00:00:00 +06 0"),
    ("Indian/Christmas", [17, 17, 17, 1, 1, 1970], "next Sunday", "Thursday 1970-01-01 17:17:17 +07 0", "Sunday 1970-01-04 00:00:00 +07 0"),
    ("America/Santo_Domingo", [17, 17, 17, 1, 291, 1970], "next Sunday", "Sunday 1970-10-18 17:17:17 EST 0", "Sunday 1970-10-25 00:30:00 -0430 1"),
    ("Pacific/Truk", [17, 17, 17, 1, 1, 1970], "next Tuesday", "Thursday 1970-01-01 17:17:17 +10 0", "Tuesday 1970-01-06 00:00:00 +10 0"),
    ("Pacific/Ponape", [17, 17, 17, 1, 1, 1970], "next Monday", "Thursday 1970-01-01 17:17:17 +11 0", "Monday 1970-01-05 00:00:00 +11 0"),
    ("America/Scoresbysund", [17, 17, 17, 1, 4099, 1970], "next Sunday", "Sunday 1981-03-22 17:17:17 -02 0", "Sunday 1981-03-29 02:00:00 +00 1"),
    ("Asia/Tehran", [17, 17, 17, 1, 2855, 1970], "next Tuesday", "Tuesday 1977-10-25 17:17:17 +04 0", "Tuesday 1977-11-01 00:00:00 +04 0"),
    ("Pacific/Tarawa", [17, 17, 17, 1, 1, 1970], "next Monday", "Thursday 1970-01-01 17:17:17 +12 0", "Monday 1970-01-05 00:00:00 +12 0"),
    ("Africa/Monrovia", [17, 17, 17, 1, 730, 1970], "next Friday", "Friday 1971-12-31 17:17:17 MMT 0", "Friday 1972-01-07 00:44:30 GMT 0"),
    ("Asia/Katmandu", [17, 17, 17, 1, 5838, 1970], "next Wednesday", "Wednesday 1985-12-25 17:17:17 +0530 0", "Wednesday 1986-01-01 00:15:00 +0545 0"),
    ("Pacific/Nauru", [17, 17, 17, 2, 3, 1979], "next Saturday +2 hours", "Saturday 1979-02-03 17:17:17 +1130 0", "Saturday 1979-02-10 02:30:00 +12 0"),
    ("Pacific/Port_Moresby", [17, 17, 17, 1, 1, 1970], "next Thursday", "Thursday 1970-01-01 17:17:17 +10 0", "Thursday 1970-01-08 00:00:00 +10 0"),
    ("America/Miquelon", [17, 17, 17, 1, 3767, 1970], "next Thursday", "Thursday 1980-04-24 17:17:17 AST 0", "Thursday 1980-05-01 01:00:00 -03 0"),
    ("Pacific/Palau", [17, 17, 17, 1, 1, 1970], "next Saturday", "Thursday 1970-01-01 17:17:17 +09 0", "Saturday 1970-01-03 00:00:00 +09 0"),
    ("Pacific/Funafuti", [17, 17, 17, 1, 1, 1970], "next Wednesday", "Thursday 1970-01-01 17:17:17 +12 0", "Wednesday 1970-01-07 00:00:00 +12 0"),
    ("Pacific/Wake", [17, 17, 17, 1, 1, 1970], "next Tuesday", "Thursday 1970-01-01 17:17:17 +12 0", "Tuesday 1970-01-06 00:00:00 +12 0"),
    ("Pacific/Wallis", [17, 17, 17, 1, 1, 1970], "next Tuesday", "Thursday 1970-01-01 17:17:17 +12 0", "Tuesday 1970-01-06 00:00:00 +12 0"),
    ("America/Paramaribo", [17, 17, 17, 1, 5381, 1970], "next Monday", "Monday 1984-09-24 17:17:17 -0330 0", "Monday 1984-10-01 00:30:00 -03 0"),
];

/// ext/date/tests/bug33415-2.phpt
#[test]
fn bug33415_2() {
    let mut bad = Vec::new();
    for &(zone, [h, i, s, m, d, y], rel, want_ts, want_res) in BUG33415_2 {
        let f = "l Y-m-d H:i:s T I";
        let stamp = mktime(h, i, s, m, d, y, zone);
        let got_ts = date(f, stamp, zone);
        let got_res = date(f, strtotime(rel, stamp, zone).unwrap(), zone);
        if got_ts != want_ts || got_res != want_res {
            bad.push(format!(
                "{zone} {rel:?}: tStamp={got_ts} (want {want_ts}); result={got_res} (want {want_res})"
            ));
        }
    }
    assert!(bad.is_empty(), "mismatches:\n{}", bad.join("\n"));
}

/// ext/date/tests/bug33578.phpt
#[test]
fn bug33578() {
    let mut out = String::new();
    for s in [
        "Oct 11",
        "11 Oct",
        "11 Oct 2005",
        "Oct11",
        "11Oct",
        "11Oct 2005",
        "11Oct2005",
    ] {
        out.push_str(&date("m/d/Y", strtotime_abs(s, "UTC").unwrap(), "UTC"));
        out.push('\n');
    }
    assert_expectf(
        &out,
        r#"10/11/%d
10/11/%d
10/11/2005
10/11/%d
10/11/%d
10/11/2005
10/11/2005"#,
    );
}

/// ext/date/tests/bug34771.phpt
#[test]
fn bug34771() {
    let mut out = String::new();
    for test in [
        "12am",
        "1am",
        "1pm",
        "12a.m.",
        "1a.m.",
        "1p.m.",
        "12:00am",
        "1:00am",
        "1:00pm",
        "12:00a.m.",
        "1:00a.m.",
        "1:00p.m.",
    ] {
        let t = strtotime_abs(&format!("2005-12-22 {test}"), "UTC").unwrap();
        out.push_str(&format!("{test:<10} => {}\n", date(DATE_ISO8601, t, "UTC")));
    }
    assert_text(
        &out,
        r#"12am       => 2005-12-22T00:00:00+0000
1am        => 2005-12-22T01:00:00+0000
1pm        => 2005-12-22T13:00:00+0000
12a.m.     => 2005-12-22T00:00:00+0000
1a.m.      => 2005-12-22T01:00:00+0000
1p.m.      => 2005-12-22T13:00:00+0000
12:00am    => 2005-12-22T00:00:00+0000
1:00am     => 2005-12-22T01:00:00+0000
1:00pm     => 2005-12-22T13:00:00+0000
12:00a.m.  => 2005-12-22T00:00:00+0000
1:00a.m.   => 2005-12-22T01:00:00+0000
1:00p.m.   => 2005-12-22T13:00:00+0000"#,
    );
}

/// ext/date/tests/bug35456.phpt
#[test]
fn bug35456() {
    let t = 1_133_216_119;
    let mut out = String::new();
    for s in ["+ 1 day", "+ 1 month", "+ 1 week"] {
        out.push_str(&date(DATE_ISO8601, strtotime(s, t, "UTC").unwrap(), "UTC"));
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2005-11-29T22:15:19+0000
2005-12-28T22:15:19+0000
2005-12-05T22:15:19+0000"#,
    );
}

/// ext/date/tests/bug35885.phpt (with the fixed NOW instead of time(); the result is a
/// round-trip property independent of the chosen time)
#[test]
fn bug35885() {
    let ts = date(DATE_ISO8601, strtotime("NOW", NOW, "UTC").unwrap(), "UTC");
    let ts2 = date(DATE_ISO8601, NOW, "UTC");
    assert_eq!(ts, ts2);
}

/// ext/date/tests/bug37017.phpt
#[test]
fn bug37017() {
    let mut out = String::new();
    for s in [
        "2006-05-12 13:00:01 America/New_York",
        "2006-05-12 13:00:00 America/New_York",
        "2006-05-12 12:59:59 America/New_York",
        "2006-05-12 12:59:59 GMT",
    ] {
        out.push_str(&format!("{}\n", strtotime_abs(s, "GMT").unwrap()));
    }
    assert_text(
        &out,
        r#"1147453201
1147453200
1147453199
1147438799"#,
    );
}

/// ext/date/tests/bug39782.phpt
#[test]
fn bug39782() {
    let mut d = DT::new("Dec 10 2006 Next Wednesday", "GMT");
    let mut out = d.format("D M j Y - H:i:s") + "\n";
    d.set_time(12, 0, 0, 0);
    out += &(d.format("D M j Y - H:i:s") + "\n");
    d.set_time(12, 0, 0, 0);
    out += &(d.format("D M j Y - H:i:s") + "\n");
    assert_text(
        &out,
        r#"Wed Dec 13 2006 - 00:00:00
Wed Dec 13 2006 - 12:00:00
Wed Dec 13 2006 - 12:00:00"#,
    );
}

/// ext/date/tests/bug41709.phpt
#[test]
fn bug41709() {
    assert_text(
        &date_parse("00.00.0000 - 00:00:00").print_r(),
        r#"Array
(
    [year] => 0
    [month] => 0
    [day] => 0
    [hour] => 0
    [minute] => 0
    [second] => 0
    [fraction] => 0
    [warning_count] => 1
    [warnings] => Array
        (
            [22] => The parsed date was invalid
        )

    [error_count] => 1
    [errors] => Array
        (
            [11] => Unexpected character
        )

    [is_localtime] => 
)"#,
    );
}

/// ext/date/tests/bug43075.phpt
#[test]
fn bug43075() {
    let d = DT::new("2007-11-01T24:34:00+00:00", "UTC");
    assert_eq!(d.format("c"), "2007-11-02T00:34:00+00:00");
}

/// ext/date/tests/bug44742.phpt
#[test]
fn bug44742() {
    let mut out = String::new();
    for s in [
        "2008-04-11 00:00:00+0000",
        "2008-04-11 00:00:00+0200",
        "2008-04-11 00:00:00+0330",
        "2008-04-11 00:00:00-0500",
        "2008-04-11 00:00:00-1130",
        "2008-04-11 00:00:00 CEST",
        "2008-04-11 00:00:00 CET",
        "2008-04-11 00:00:00 UTC",
        "2008-04-11 00:00:00 America/New_York",
        "2008-04-11 00:00:00 Europe/Oslo",
        "2008-04-11 00:00:00 Asia/Singapore",
    ] {
        let d = DT::new(s, "Europe/London");
        out.push_str(&V::Int(tz_offset_get(&get_timezone(&d), &d.t)).var_dump());
    }
    assert_text(
        &out,
        r#"int(0)
int(7200)
int(12600)
int(-18000)
int(-41400)
int(7200)
int(3600)
int(0)
int(-14400)
int(7200)
int(28800)"#,
    );
}

/// ext/date/tests/bug45682.phpt
#[test]
fn bug45682() {
    let date = DT::new("28-July-2008", "UTC");
    let other = DT::new("31-July-2008", "UTC");
    assert_expectf(
        &interval_var_dump(&date.diff(&other), 3),
        r#"object(DateInterval)#%d (%d) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(3)
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
  int(3)
  ["from_string"]=>
  bool(false)
}"#,
    );
}

/// ext/date/tests/bug48058.phpt
#[test]
fn bug48058() {
    let tran = php::timezone_transitions_get(&tz("Europe/London"), i64::MIN, i64::from(i32::MAX));
    let t0 = &tran[0];
    let v = V::Arr(vec![
        (K::S("ts".into()), V::Int(t0.ts)),
        (K::S("time".into()), V::Str(t0.time())),
        (K::S("offset".into()), V::Int(i64::from(t0.offset))),
        (K::S("isdst".into()), V::Bool(t0.isdst)),
        (K::S("abbr".into()), V::Str(t0.abbr.clone())),
    ]);
    let dt = DT::new("28 Feb 2008 12:00:00 +10000000000 years", "Europe/London");
    let out = v.var_dump() + &dt.format(DATE_ISO8601);
    assert_expectf(
        &out,
        r#"array(5) {
  ["ts"]=>
  int(-%d)
  ["time"]=>
  string(%d) "%s"
  ["offset"]=>
  int(%s)
  ["isdst"]=>
  bool(%s)
  ["abbr"]=>
  string(%d) "%s"
}
10000002008-02-28T12:00:00+0000"#,
    );
}

/// ext/date/tests/bug49081.phpt
#[test]
fn bug49081() {
    let d1 = DT::new("2010-01-01 06:00:00", "Europe/Berlin");
    let d2 = DT::new("2010-01-31 10:00:00", "Europe/Berlin");
    let d = d1.diff(&d2);
    assert_text(
        &V::Obj("DateInterval".into(), 3, interval_props(&d)).print_r(),
        r#"DateInterval Object
(
    [y] => 0
    [m] => 0
    [d] => 30
    [h] => 4
    [i] => 0
    [s] => 0
    [f] => 0
    [invert] => 0
    [days] => 30
    [from_string] => 
)"#,
    );
}

/// ext/date/tests/bug50392.phpt
#[test]
fn bug50392() {
    let base = "2009-03-01 18:00:00";
    let mut out = String::new();
    for i in 0..8 {
        let s = format!("{base}.{}", i.to_string().repeat(i));
        out.push_str(&format!("{s}\n- "));
        let result = date_parse_from_format("Y-m-d H:i:s.u", &s);
        match get(&result, "fraction") {
            V::Float(f) if *f != 0.0 => out.push_str(&format!("{f}")),
            _ => out.push('X'),
        }
        out.push('\n');
        if let V::Arr(errors) = get(&result, "errors") {
            for (_, e) in errors {
                if let V::Str(e) = e {
                    out.push_str(&format!("- {e}\n"));
                }
            }
        }
        out.push('\n');
    }
    assert_text(
        &out,
        r#"2009-03-01 18:00:00.
- X
- Not enough data available to satisfy format

2009-03-01 18:00:00.1
- 0.1

2009-03-01 18:00:00.22
- 0.22

2009-03-01 18:00:00.333
- 0.333

2009-03-01 18:00:00.4444
- 0.4444

2009-03-01 18:00:00.55555
- 0.55555

2009-03-01 18:00:00.666666
- 0.666666

2009-03-01 18:00:00.7777777
- 0.777777
- Trailing data"#,
    );
}

/// ext/date/tests/bug51866.phpt
#[test]
fn bug51866() {
    let mut out = String::new();
    for (format, s) in [
        ("Y-m-d", "2001-11-29 13:20:01"),
        ("Y-m-d+", "2001-11-29 13:20:01"),
        ("Y-m-d +", "2001-11-29 13:20:01"),
        ("Y-m-d+", "2001-11-29"),
        ("Y-m-d +", "2001-11-29"),
        ("Y-m-d +", "2001-11-29 "),
    ] {
        out.push_str(&vd_str(format));
        out.push_str(&vd_str(s));
        match DT::from_format(format, s, None, "UTC") {
            Some(d) => out.push_str(&d.var_dump("DateTime", 1)),
            None => out.push_str(&vd_bool_false()),
        }
        match last_errors(Some(format), s) {
            Some(v) => out.push_str(&v.var_dump()),
            None => out.push_str(&vd_bool_false()),
        }
        out.push_str("\n\n");
    }
    assert_expectf(
        &out,
        r#"string(5) "Y-m-d"
string(19) "2001-11-29 13:20:01"
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
    [10]=>
    string(13) "Trailing data"
  }
}


string(6) "Y-m-d+"
string(19) "2001-11-29 13:20:01"
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2001-11-29 %d:%d:%d.%d"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(%d) "%s"
}
array(4) {
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [10]=>
    string(13) "Trailing data"
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
}


string(7) "Y-m-d +"
string(19) "2001-11-29 13:20:01"
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2001-11-29 %d:%d:%d.%d"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(%d) "%s"
}
array(4) {
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [11]=>
    string(13) "Trailing data"
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
}


string(6) "Y-m-d+"
string(10) "2001-11-29"
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2001-11-29 %d:%d:%d.%d"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(%d) "%s"
}
bool(false)


string(7) "Y-m-d +"
string(10) "2001-11-29"
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
    [10]=>
    string(%d) "Not enough data available to satisfy format"
  }
}


string(7) "Y-m-d +"
string(11) "2001-11-29 "
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2001-11-29 %d:%d:%d.%d"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(%d) "%s"
}
bool(false)"#,
    );
}

/// ext/date/tests/bug52063.phpt
#[test]
fn bug52063() {
    let a = DT::new("2009-01-01", "Europe/Lisbon");
    assert_eq!(a.format(DATE_COOKIE), "Thursday, 01-Jan-2009 00:00:00 WET");
    let a = DT::try_new("2009-01-01", None, "Europe/Lisbon").unwrap();
    assert_eq!(a.format(DATE_COOKIE), "Thursday, 01-Jan-2009 00:00:00 WET");
}

/// ext/date/tests/bug52480.phpt
#[test]
fn bug52480() {
    let mut expected = php::date_interval_create("P30D").unwrap();
    expected.diff.days = 30;
    let expected = interval_props(&expected);
    let mut wrong = Vec::new();
    for timezone in timezone_identifiers_list(TZ_ALL) {
        let start = DT::new_in("2017-03-01", &timezone, "UTC");
        let end = DT::new_in("2017-03-31", &timezone, "UTC");
        if interval_props(&start.diff(&end)) != expected {
            wrong.push(timezone);
        }
    }
    assert!(wrong.is_empty(), "Wrong result for {wrong:?}");
}

/// ext/date/tests/bug53502.phpt (the script only checks that parsing succeeds)
#[test]
fn bug53502() {
    for _ in 0..10 {
        assert!(strtotime("Monday 00:00 Europe/Paris", NOW, "UTC").is_some());
    }
}

/// ext/date/tests/bug54851.phpt (with the fixed NOW for the "now"-relative parts)
#[test]
fn bug54851() {
    let z = "UTC";
    let b = |v: bool| V::Bool(v).var_dump();
    let mut out = String::new();
    let date = DT::new("2011-05-17T22:14:12", z);
    let date2 =
        DT::from_format("D H i s", &format!("{} 0 00 00", date.format("D")), None, z).unwrap();
    out += &format!("{}\n{}\n", date.format("r"), date2.format("r"));
    out += &b(date.format("D") == date2.format("D"));

    let pre = DT::from_format("!D d M Y", "Fri 17 may 2011", None, z).unwrap();
    let post = DT::from_format("!d M Y D", "17 may 2011 Fri", None, z).unwrap();
    out += &format!("{}\n{}\n", pre.format("r"), post.format("r"));
    out += &b(pre.format("Y-m-d") == post.format("Y-m-d"));

    let date1 = DT::new("Tuesday", z);
    let date2 = DT::from_format("D H i s", "Tuesday 0 00 00", None, z).unwrap();
    out += &format!("{}\n{}\n", date1.format("r"), date2.format("r"));
    out += &b(date1.format("D") == date2.format("D"));

    for s in ["Fri 19 November 2011", "Sat 19 November 2011"] {
        let date1 = DT::from_format("!D d M Y", s, None, z).unwrap();
        let date2 = DT::new(s, z);
        out += &format!("{}\n{}\n", date1.format("r"), date2.format("r"));
        out += &b(date1.format("Y-m-d") == date2.format("Y-m-d"));
    }
    assert_expectf(
        &out,
        r#"Tue, 17 May 2011 22:14:12 +0000
Tue, %d %s %d 00:00:00 +0000
bool(true)
Fri, 20 May 2011 00:00:00 +0000
Fri, 20 May 2011 00:00:00 +0000
bool(true)
Tue, %d %s %d 00:00:00 +0000
Tue, %d %s %d 00:00:00 +0000
bool(true)
Fri, 25 Nov 2011 00:00:00 +0000
Fri, 25 Nov 2011 00:00:00 +0000
bool(true)
Sat, 19 Nov 2011 00:00:00 +0000
Sat, 19 Nov 2011 00:00:00 +0000
bool(true)"#,
    );
}

/// ext/date/tests/bug63740.phpt
#[test]
fn bug63740() {
    let mut out = String::new();
    for d in 4..=14 {
        let dt = DT::new(&format!("2015-07-{d:02} 00:00 UTC"), "UTC");
        let dtn = modified(&dt, "this week");
        out.push_str(&format!(
            "{} → {}\n",
            dt.format("D Y-m-d H:i"),
            dtn.format("D Y-m-d H:i")
        ));
    }
    assert_text(
        &out,
        r#"Sat 2015-07-04 00:00 → Mon 2015-06-29 00:00
Sun 2015-07-05 00:00 → Mon 2015-06-29 00:00
Mon 2015-07-06 00:00 → Mon 2015-07-06 00:00
Tue 2015-07-07 00:00 → Mon 2015-07-06 00:00
Wed 2015-07-08 00:00 → Mon 2015-07-06 00:00
Thu 2015-07-09 00:00 → Mon 2015-07-06 00:00
Fri 2015-07-10 00:00 → Mon 2015-07-06 00:00
Sat 2015-07-11 00:00 → Mon 2015-07-06 00:00
Sun 2015-07-12 00:00 → Mon 2015-07-06 00:00
Mon 2015-07-13 00:00 → Mon 2015-07-13 00:00
Tue 2015-07-14 00:00 → Mon 2015-07-13 00:00"#,
    );
}

/// ext/date/tests/bug66545.phpt
#[test]
fn bug66545() {
    let z = "Europe/Paris";
    let debut = mktime(0, 0, 0, 10, 27, 2013, z);
    let fin = mktime(23, 59, 59, 11, 10, 2013, z);
    let mut d1 = DT::new_in("now", z, z);
    let mut d2 = DT::new_in("now", z, z);
    d1.set_timestamp(debut);
    d2.set_timestamp(fin);
    let diff = d1.diff(&d2);
    assert_text(
        &V::Obj("DateInterval".into(), 5, interval_props(&diff)).print_r(),
        r#"DateInterval Object
(
    [y] => 0
    [m] => 0
    [d] => 14
    [h] => 23
    [i] => 59
    [s] => 59
    [f] => 0
    [invert] => 0
    [days] => 14
    [from_string] => 
)"#,
    );
}

/// ext/date/tests/bug67118_2.phpt (the constructor retry of the subclass)
#[test]
fn bug67118_2() {
    let utc = php::timezone_open("UTC").unwrap();
    let date = "12 Sep 2007 15:49:12 UT";
    let mut out = String::from("First try\n");
    let d = match DT::try_new(date, Some(&utc), "Europe/Paris") {
        Ok(d) => d,
        Err(_) => {
            out.push_str("Second try\n");
            DT::try_new(&format!("{date}C"), Some(&utc), "Europe/Paris").unwrap()
        }
    };
    out.push_str(&d.var_dump("Foo", 1));
    out.push_str("Done\n");
    assert_text(
        &out,
        r#"First try
Second try
object(Foo)#1 (3) {
  ["date"]=>
  string(26) "2007-09-12 15:49:12.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
Done"#,
    );
}

/// ext/date/tests/bug68406.phpt
#[test]
fn bug68406() {
    let tz2 = php::timezone_open("Europe/Berlin").unwrap();
    let d = DT::new_in("2014-12-24 13:00:00", "Europe/Berlin", "UTC");
    let dtz = get_timezone(&d);
    let mut out = tz_var_dump(&dtz, 4) + &tz_var_dump(&tz2, 2);
    // DateTimeZone objects compare equal when type and name match.
    let same = dtz.zone_type() == tz2.zone_type() && dtz.name() == tz2.name();
    out.push_str(if same { "yes" } else { "no" });
    assert_text(
        &out,
        r#"object(DateTimeZone)#4 (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/Berlin"
}
object(DateTimeZone)#2 (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/Berlin"
}
yes"#,
    );
}

/// ext/date/tests/bug71700.phpt
#[test]
fn bug71700() {
    let z = "Europe/Paris";
    let date1 = DT::new("2016-03-01", z);
    let date2 = DT::new("2016-03-31", z);
    let diff = php::date_diff(&date1.t, &date2.t, true);
    assert_expectf(
        &interval_var_dump(&diff, 3),
        r#"object(DateInterval)#3 (%d) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(30)
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
  int(30)
  ["from_string"]=>
  bool(false)
}"#,
    );
}

/// ext/date/tests/bug73489.phpt
#[test]
fn bug73489() {
    for _ in 0..2 {
        let mut datetime = DT::new_in("2016-11-09 20:00:00", "UTC", "UTC");
        assert_eq!(datetime.timestamp(), 1_478_721_600);
        datetime.set_timezone("-03:00");
        let _ = datetime.timestamp();
        datetime.set_timezone("-03:00");
        assert_eq!(datetime.timestamp(), 1_478_721_600);
    }
}

/// ext/date/tests/bug74274.phpt
#[test]
fn bug74274() {
    let start = DT::new_in("2018-10-28 00:00:00", "Europe/Paris", "UTC");
    let end = DT::new_in("2018-10-29 23:00:00", "Europe/Paris", "UTC");
    assert_text(
        &V::Obj("DateInterval".into(), 4, interval_props(&start.diff(&end))).print_r(),
        r#"DateInterval Object
(
    [y] => 0
    [m] => 0
    [d] => 1
    [h] => 23
    [i] => 0
    [s] => 0
    [f] => 0
    [invert] => 0
    [days] => 1
    [from_string] => 
)"#,
    );
}

/// ext/date/tests/bug75851.phpt
#[test]
fn bug75851() {
    let f = format!("{DATE_ATOM}\n{DATE_RFC2822}\nc\nr\no\ny\nY\nU\n\n");
    let mut out = String::new();
    for ts in [
        i64::MIN,
        67_767_976_233_532_799,
        67_767_976_233_532_800,
        i64::MAX,
    ] {
        out.push_str(&date(&f, ts, "UTC"));
    }
    assert_text(
        &out,
        r#"-292277022657-01-27T08:29:52+00:00
Sun, 27 Jan -292277022657 08:29:52 +0000
-292277022657-01-27T08:29:52+00:00
Sun, 27 Jan -292277022657 08:29:52 +0000
-292277022657
-57
-292277022657
-9223372036854775808

2147483647-12-31T23:59:59+00:00
Tue, 31 Dec 2147483647 23:59:59 +0000
2147483647-12-31T23:59:59+00:00
Tue, 31 Dec 2147483647 23:59:59 +0000
2147483648
47
2147483647
67767976233532799

2147483648-01-01T00:00:00+00:00
Wed, 01 Jan 2147483648 00:00:00 +0000
2147483648-01-01T00:00:00+00:00
Wed, 01 Jan 2147483648 00:00:00 +0000
2147483648
48
2147483648
67767976233532800

292277026596-12-04T15:30:07+00:00
Sun, 04 Dec 292277026596 15:30:07 +0000
292277026596-12-04T15:30:07+00:00
Sun, 04 Dec 292277026596 15:30:07 +0000
292277026596
96
292277026596
9223372036854775807"#,
    );
}

/// ext/date/tests/bug76770.phpt
#[test]
fn bug76770() {
    let d = DT::from_format("U H", "3600 01", None, "UTC").unwrap();
    assert_eq!(d.timestamp(), 3600);
}

/// ext/date/tests/bug78139.phpt
#[test]
fn bug78139() {
    let mut out = String::new();
    for s in ["x", "x UTC", "xx UTC", "xUTC", "UTCx", "UTC xx"] {
        out.push_str(&format!("Parsing '{s}':\n"));
        match php::timezone_open(s) {
            Ok(z) => out.push_str(&tz_var_dump(&z, 1)),
            Err(m) => {
                out.push_str(&vd_bool_false());
                out.push_str(&format!(
                    "DateInvalidTimeZoneException: DateTimeZone::__construct(): {m}\n"
                ));
            }
        }
        out.push_str("\n\n");
    }
    assert_text(
        &out,
        r#"Parsing 'x':
object(DateTimeZone)#1 (2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(1) "X"
}


Parsing 'x UTC':
bool(false)
DateInvalidTimeZoneException: DateTimeZone::__construct(): Unknown or bad timezone (x UTC)


Parsing 'xx UTC':
bool(false)
DateInvalidTimeZoneException: DateTimeZone::__construct(): Unknown or bad timezone (xx UTC)


Parsing 'xUTC':
bool(false)
DateInvalidTimeZoneException: DateTimeZone::__construct(): Unknown or bad timezone (xUTC)


Parsing 'UTCx':
bool(false)
DateInvalidTimeZoneException: DateTimeZone::__construct(): Unknown or bad timezone (UTCx)


Parsing 'UTC xx':
bool(false)
DateInvalidTimeZoneException: DateTimeZone::__construct(): Unknown or bad timezone (UTC xx)"#,
    );
}

/// ext/date/tests/bug79452.phpt
#[test]
fn bug79452() {
    for z in [NY, "Asia/Tokyo"] {
        let from = DT::new("2019-06-01", z);
        let to = DT::new("2019-10-01", z);
        assert_eq!(from.diff(&to).diff.m, 4, "{z}");
    }
}

/// ext/date/tests/bug80610.phpt
#[test]
fn bug80610() {
    let mut out = String::new();
    for spec in ["PT20800M", "PT20715M", "PT20700M"] {
        let mut e = DT::from_format(
            "Y-m-d H:i:s",
            "2020-04-12 12:00:00",
            Some("Europe/Berlin"),
            "UTC",
        )
        .unwrap();
        let interval = php::date_interval_create(spec).unwrap();
        e.sub_interval(&interval);
        e.add_interval(&interval);
        out.push_str(&format!("easter {}\n", e.format("Y-m-d H:i:s")));
    }
    assert_text(
        &out,
        r#"easter 2020-04-12 12:00:00
easter 2020-04-12 12:00:00
easter 2020-04-12 12:00:00"#,
    );
}

/// ext/date/tests/bug81097.phpt
#[test]
fn bug81097() {
    let d = php::timezone_open("+01:45:30").unwrap();
    assert_expectf(
        &tz_var_dump(&d, 1),
        r#"object(DateTimeZone)#%d (%d) {
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(9) "+01:45:30"
}"#,
    );
}

/// ext/date/tests/bug81504.phpt
#[test]
fn bug81504() {
    let z = tz("Europe/Amsterdam");
    let begin = strtotime_abs("1996-01-01", "UTC").unwrap();
    let end = strtotime_abs("1997-12-31", "UTC").unwrap();
    let mut out = String::new();
    for tr in php::timezone_transitions_get(&z, begin, end) {
        out.push_str(&format!("{} {} {}\n", tr.time(), tr.offset, tr.abbr));
    }
    assert_text(
        &out,
        r#"1996-01-01T00:00:00+00:00 3600 CET
1996-03-31T01:00:00+00:00 7200 CEST
1996-10-27T01:00:00+00:00 3600 CET
1997-03-30T01:00:00+00:00 7200 CEST
1997-10-26T01:00:00+00:00 3600 CET"#,
    );
}

/// ext/date/tests/date-lenient-create.phpt
#[test]
fn date_lenient_create() {
    let utc = Some("UTC");
    let pr = |d: Option<DT>| match d {
        Some(mut d) => {
            d.set_time(0, 0, 0, 0);
            V::Obj("DateTime".into(), 2, d.props()).print_r()
        }
        None => String::new(),
    };
    let errs = |f: &str, s: &str| {
        last_errors(Some(f), s)
            .map(|v| v.print_r())
            .unwrap_or_default()
    };
    let mut out = String::from("==\n");
    let date = "06/08/04 12:00";
    out += &pr(DT::from_format("m/d/y", date, utc, "UTC"));
    out += &errs("m/d/y", date);
    out += "==\n";
    for f in ["m/d/y+", "+m/d/y", "m/d/y++"] {
        out += &pr(DT::from_format(f, date, utc, "UTC"));
        out += &errs(f, date);
        out += "==\n";
    }
    let date = "06/08/04";
    for f in ["m/d/y+", "+m/d/y"] {
        out += &pr(DT::from_format(f, date, utc, "UTC"));
        out += &errs(f, date);
        out += "==\n";
    }
    assert_text(
        &out,
        r#"==
Array
(
    [warning_count] => 0
    [warnings] => Array
        (
        )

    [error_count] => 1
    [errors] => Array
        (
            [8] => Trailing data
        )

)
==
DateTime Object
(
    [date] => 2004-06-08 00:00:00.000000
    [timezone_type] => 3
    [timezone] => UTC
)
Array
(
    [warning_count] => 1
    [warnings] => Array
        (
            [8] => Trailing data
        )

    [error_count] => 0
    [errors] => Array
        (
        )

)
==
DateTime Object
(
    [date] => 2004-06-08 00:00:00.000000
    [timezone_type] => 3
    [timezone] => UTC
)
Array
(
    [warning_count] => 1
    [warnings] => Array
        (
            [8] => Trailing data
        )

    [error_count] => 0
    [errors] => Array
        (
        )

)
==
DateTime Object
(
    [date] => 2004-06-08 00:00:00.000000
    [timezone_type] => 3
    [timezone] => UTC
)
Array
(
    [warning_count] => 1
    [warnings] => Array
        (
            [8] => Trailing data
        )

    [error_count] => 0
    [errors] => Array
        (
        )

)
==
DateTime Object
(
    [date] => 2004-06-08 00:00:00.000000
    [timezone_type] => 3
    [timezone] => UTC
)
==
DateTime Object
(
    [date] => 2004-06-08 00:00:00.000000
    [timezone_type] => 3
    [timezone] => UTC
)
=="#,
    );
}

/// ext/date/tests/date_add_basic.phpt
#[test]
fn date_add_basic() {
    let format = "Y-m-d H:i:s";
    let mut d = DT::new("2008-01-01 12:25", "UTC");
    let mut out = String::from("*** Testing date_add() : basic functionality ***\n");
    out.push_str(&vd_str(&d.format(format)));
    for interval in ["P3Y6M4DT12H30M5S", "P0D", "P2DT1M", "P1Y2MT23H43M150S"] {
        d.add(interval);
        out.push_str(&vd_str(&d.format(format)));
    }
    assert_text(
        &out,
        r#"*** Testing date_add() : basic functionality ***
string(19) "2008-01-01 12:25:00"
string(19) "2011-07-06 00:55:05"
string(19) "2011-07-06 00:55:05"
string(19) "2011-07-08 00:56:05"
string(19) "2012-09-09 00:41:35""#,
    );
}

/// ext/date/tests/date_create_basic.phpt (with the fixed NOW; the "now"-dependent
/// parts are wildcards in the EXPECTF)
#[test]
fn date_create_basic() {
    let z = "Europe/London";
    let mut out = String::from("*** Testing date_create() : basic functionality ***\n");
    for s in [
        "now",
        "GMT",
        "2005-07-14 22:30:41",
        "2005-07-14 22:30:41 GMT",
    ] {
        out.push_str(&DT::new(s, z).var_dump("DateTime", 1));
    }
    assert_expectf(
        &out,
        r#"*** Testing date_create() : basic functionality ***
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "%s"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "%s"
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

/// ext/date/tests/date_format_timezone.phpt
#[test]
fn date_format_timezone() {
    let mut out = String::new();
    for zone in ["UTC", "Europe/London", "Europe/Berlin", "America/Chicago"] {
        out.push_str(&format!("{zone}\n"));
        let d = DT::new("2020-03-10 22:30:41", zone);
        for f in ["O", "P", "p"] {
            out.push_str(&vd_str(&d.format(f)));
        }
    }
    out.push_str("Z\n");
    let d = DT::new("2020-03-10 22:30:41Z", "America/Chicago");
    out.push_str(&vd_str(&d.format("p")));
    assert_text(
        &out,
        r#"UTC
string(5) "+0000"
string(6) "+00:00"
string(1) "Z"
Europe/London
string(5) "+0000"
string(6) "+00:00"
string(6) "+00:00"
Europe/Berlin
string(5) "+0100"
string(6) "+01:00"
string(6) "+01:00"
America/Chicago
string(5) "-0500"
string(6) "-05:00"
string(6) "-05:00"
Z
string(1) "Z""#,
    );
}

/// ext/date/tests/date_interval_format.phpt
#[test]
fn date_interval_format() {
    let i = php::date_interval_create("P2Y4DT6H8M").unwrap();
    assert_eq!(interval_format(&i, "%d days"), "4 days");
    let i = php::date_interval_create("P32D").unwrap();
    assert_eq!(interval_format(&i, "%d days"), "32 days");
}

/// ext/date/tests/date_modify-2.phpt
#[test]
fn date_modify_2() {
    let mut d = DT::new("2005-07-18 22:10:00 +0400", "GMT");
    let mut out = d.format("D, d M Y H:i:s T") + "\n";
    assert!(d.modify("+1 hour"));
    out += &(d.format("D, d M Y H:i:s T") + "\n");
    assert_text(
        &out,
        r#"Mon, 18 Jul 2005 22:10:00 GMT+0400
Mon, 18 Jul 2005 23:10:00 GMT+0400"#,
    );
}

/// ext/date/tests/date_period-immutable.phpt
#[test]
fn date_period_immutable() {
    let z = "UTC";
    let di = php::date_interval_create_from_date_string("first day of next month").unwrap();
    let de = DT::new("2008-03-31", z);
    let mut out = String::new();
    for (class, immutable) in [("DateTimeImmutable", true), ("DateTime", false)] {
        let db = DT::new("2008-01-01", z);
        for t in period(&db.t, &di.diff, Some(&de.t), 0, true, false) {
            let mut dt = dt_of(t, z);
            out.push_str(&format!("{class}\n"));
            out.push_str(&dt.format("l Y-m-d\n"));
            if immutable {
                out.push_str(&modified(&dt, "3 tuesday").format("l Y-m-d\n"));
            } else {
                assert!(dt.modify("3 tuesday"));
                out.push_str(&dt.format("l Y-m-d\n"));
            }
            out.push_str(&dt.format("l Y-m-d\n\n"));
        }
    }
    assert_text(
        &out,
        r#"DateTimeImmutable
Tuesday 2008-01-01
Tuesday 2008-01-15
Tuesday 2008-01-01

DateTimeImmutable
Friday 2008-02-01
Tuesday 2008-02-19
Friday 2008-02-01

DateTimeImmutable
Saturday 2008-03-01
Tuesday 2008-03-18
Saturday 2008-03-01

DateTime
Tuesday 2008-01-01
Tuesday 2008-01-15
Tuesday 2008-01-15

DateTime
Friday 2008-02-01
Tuesday 2008-02-19
Tuesday 2008-02-19

DateTime
Saturday 2008-03-01
Tuesday 2008-03-18
Tuesday 2008-03-18"#,
    );
}

/// ext/date/tests/date_period_microseconds.phpt
#[test]
fn date_period_microseconds() {
    let z = "UTC";
    let f = "Y-m-d H:i:s.u";
    let start = DT::new("2010-06-07T01:02:03.456789", z);
    let interval = php::date_interval_create("P1D").unwrap();
    let mut out = String::new();
    for (end, include_end) in [
        ("2010-06-10T01:02:03.456789", false),
        ("2010-06-10T01:02:03.456789", true),
        ("2010-06-10T01:02:03.456790", false),
        ("2010-06-10T01:02:03.456788", true),
    ] {
        let end = DT::new(end, z);
        out.push_str(&format!(
            "from {} to {} ({})\n",
            start.format(f),
            end.format(f),
            if include_end {
                "inclusive"
            } else {
                "exclusive"
            }
        ));
        for t in period(&start.t, &interval.diff, Some(&end.t), 0, true, include_end) {
            out.push_str(&dt_of(t, z).format(f));
            out.push('\n');
        }
    }
    assert_text(
        &out,
        r#"from 2010-06-07 01:02:03.456789 to 2010-06-10 01:02:03.456789 (exclusive)
2010-06-07 01:02:03.456789
2010-06-08 01:02:03.456789
2010-06-09 01:02:03.456789
from 2010-06-07 01:02:03.456789 to 2010-06-10 01:02:03.456789 (inclusive)
2010-06-07 01:02:03.456789
2010-06-08 01:02:03.456789
2010-06-09 01:02:03.456789
2010-06-10 01:02:03.456789
from 2010-06-07 01:02:03.456789 to 2010-06-10 01:02:03.456790 (exclusive)
2010-06-07 01:02:03.456789
2010-06-08 01:02:03.456789
2010-06-09 01:02:03.456789
2010-06-10 01:02:03.456789
from 2010-06-07 01:02:03.456789 to 2010-06-10 01:02:03.456788 (inclusive)
2010-06-07 01:02:03.456789
2010-06-08 01:02:03.456789
2010-06-09 01:02:03.456789"#,
    );
}

/// ext/date/tests/date_sunrise_and_sunset_basic.phpt (partial, with the fixed NOW: the
/// EXPECTF only checks the shape of the output)
#[test]
fn date_sunrise_and_sunset_basic() {
    let utc = tz("UTC");
    for sunset in [false, true] {
        let r = php_timelib::sunrise_sunset(
            NOW,
            SunFormat::String,
            38.4,
            -9.0,
            90.0,
            Some(1.0),
            &utc,
            sunset,
        );
        let Some(SunResult::String(s)) = r else {
            panic!("{r:?}");
        };
        assert_expectf(&s, "%d:%d");
        // date_sunrise(time()) with the ini defaults returns a string too.
        let r = php_timelib::sunrise_sunset(
            NOW,
            SunFormat::String,
            31.7667,
            35.2333,
            90.833333,
            None,
            &utc,
            sunset,
        );
        assert!(matches!(r, Some(SunResult::String(_))), "{r:?}");
    }
}

/// ext/date/tests/date_time_fractions.phpt (the first line does not depend on the
/// current time: `($ms = 0)` is an assignment)
#[test]
fn date_time_fractions() {
    let z = "UTC";
    let f = "Y-m-d H:i:s.u";
    let mut out = String::from("microseconds = true\n");
    out += &format!("{}\n\n", DT::new("2016-10-03 12:47:18.819313", z).format(f));
    let dt = DT::new("2016-10-03 12:47:18.819210", z);
    out += &format!("{}\n", modified(&dt, "+1 day").format(f));
    let dt = DT::new("2016-10-03 12:47:18.081921", z);
    out += &format!("{}\n", modified(&dt, "-3 months").format(f));
    out += "\n";
    for m in ["yesterday", "noon", "10 weekday"] {
        let dt = DT::new("2016-10-03 12:47:18.081921", z);
        out += &format!("{}\n", modified(&dt, m).format(f));
    }
    let dt1 = DT::new("2016-10-03 13:20:07.103123", z);
    let dt2 = DT::new("2016-10-03 13:20:07.481312", z);
    let diff = dt1.diff(&dt2);
    out += &interval_var_dump(&diff, 3);
    let mut dt0 = dt1.clone();
    dt0.sub_interval(&diff);
    let mut dt3 = dt2.clone();
    dt3.add_interval(&diff);
    let mut dt4 = dt3.clone();
    dt4.add_interval(&diff);
    for d in [&dt0, &dt1, &dt2, &dt3, &dt4] {
        out += &format!("{}\n", d.format(f));
    }
    assert_expectf(
        &out,
        r#"microseconds = true
2016-10-03 12:47:18.819313

2016-10-04 12:47:18.819210
2016-07-03 12:47:18.081921

2016-10-02 00:00:00.000000
2016-10-03 12:00:00.000000
2016-10-17 12:47:18.081921
object(DateInterval)#%d (%d) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(0)
  ["h"]=>
  int(0)
  ["i"]=>
  int(0)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0.378189)
  ["invert"]=>
  int(0)
  ["days"]=>
  int(0)
  ["from_string"]=>
  bool(false)
}
2016-10-03 13:20:06.724934
2016-10-03 13:20:07.103123
2016-10-03 13:20:07.481312
2016-10-03 13:20:07.859501
2016-10-03 13:20:08.237690"#,
    );
}

/// ext/date/tests/date_time_set_basic1.phpt
#[test]
fn date_time_set_basic1() {
    let mut datetime = DT::new("2009-01-31 15:14:10", "Europe/London");
    let mut out = String::from("*** Testing date_time_set() : basic functionality ***\n");
    out += &format!("Initial date: {}\n", datetime.format(DATE_RFC2822));
    for (n, (h, i, s)) in [
        (17, 20, 0),
        (19, 5, 59),
        (24, 10, 0),
        (47, 35, 47),
        (54, 25, 0),
    ]
    .into_iter()
    .enumerate()
    {
        datetime.set_time(h, i, s, 0);
        out += &format!(
            "After modification{} {}\n",
            n + 1,
            datetime.format(DATE_RFC2822)
        );
    }
    assert_text(
        &out,
        r#"*** Testing date_time_set() : basic functionality ***
Initial date: Sat, 31 Jan 2009 15:14:10 +0000
After modification1 Sat, 31 Jan 2009 17:20:00 +0000
After modification2 Sat, 31 Jan 2009 19:05:59 +0000
After modification3 Sun, 01 Feb 2009 00:10:00 +0000
After modification4 Mon, 02 Feb 2009 23:35:47 +0000
After modification5 Wed, 04 Feb 2009 06:25:00 +0000"#,
    );
}

/// ext/date/tests/default-timezone-1.phpt
#[test]
fn default_timezone_1() {
    assert_eq!(
        strtotime_abs("2005-06-18 22:15:44", "GMT"),
        Some(1_119_132_944)
    );
}

/// ext/date/tests/getdate_variation2.phpt
#[test]
fn getdate_variation2() {
    let zone = "Asia/Calcutta";
    let getdate = |ts: i64| {
        let mut t = Time {
            tz_info: Some(tz(zone)),
            zone_type: ZoneType::Id,
            ..Time::new()
        };
        t.unixtime2local(ts);
        let s = |k: &str, v: V| (K::S(k.into()), v);
        V::Arr(vec![
            s("seconds", V::Int(t.s)),
            s("minutes", V::Int(t.i)),
            s("hours", V::Int(t.h)),
            s("mday", V::Int(t.d)),
            s("wday", V::Int(php_timelib::day_of_week(t.y, t.m, t.d))),
            s("mon", V::Int(t.m)),
            s("year", V::Int(t.y)),
            s("yday", V::Int(php_timelib::day_of_year(t.y, t.m, t.d))),
            s(
                "weekday",
                V::Str(php_timelib::format::full_day_name(t.y, t.m, t.d).into()),
            ),
            s("month", V::Str(dt_of(t.clone(), zone).format("F"))),
            (K::I(0), V::Int(ts)),
        ])
    };
    let mut out = String::from("*** Testing getdate() : usage variation ***\n");
    for (key, value) in [
        ("octal 05", 0o5),
        ("octal 010", 0o10),
        ("octal -010", -0o10),
    ] {
        out += &format!("\n--{key}--\n");
        out += &getdate(value).var_dump();
    }
    assert_text(
        &out,
        r#"*** Testing getdate() : usage variation ***

--octal 05--
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

--octal 010--
array(11) {
  ["seconds"]=>
  int(8)
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
  int(8)
}

--octal -010--
array(11) {
  ["seconds"]=>
  int(52)
  ["minutes"]=>
  int(29)
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
  int(-8)
}"#,
    );
}

/// ext/date/tests/gh14383.phpt
#[test]
fn gh14383() {
    // DateTime::createFromTimestamp(float) (`php_date_initialize_from_ts_double`).
    let create_from_timestamp = |ts: f64| {
        let mut sec = ts.trunc() as i64;
        let mut usec = ((ts % 1.0) * 1_000_000.0).round() as i64;
        if usec.abs() == 1_000_000 {
            sec += if usec > 0 { 1 } else { -1 };
            usec = 0;
        }
        if usec < 0 {
            sec -= 1;
            usec += 1_000_000;
        }
        let mut t = Time {
            zone_type: ZoneType::Offset,
            ..Time::new()
        };
        t.unixtime2gmt(sec);
        t.update_ts(None);
        t.us = usec;
        dt_of(t, "UTC")
    };
    let cases = [
        (0.999_999_0, "0.999_999_0"),
        (0.999_999_1, "0.999_999_1"),
        (0.999_999_8, "0.999_999_8"),
        (0.999_999_9, "0.999_999_9"),
        (1.000_000_0, "1.000_000_0"),
        (1.000_000_1, "1.000_000_1"),
        (1.000_000_8, "1.000_000_8"),
        (1.000_000_9, "1.000_000_9"),
        (1.000_001_0, "1.000_001_0"),
        (1.000_001_1, "1.000_001_1"),
        (1.000_001_8, "1.000_001_8"),
        (1.000_001_9, "1.000_001_9"),
    ];
    let mut out = String::from("plus:\n");
    for (usec, label) in cases {
        out += &format!("{label}: {}\n", create_from_timestamp(usec).format("s.u"));
    }
    out += "\nminus:\n";
    for (usec, label) in cases {
        out += &format!("-{label}: {}\n", create_from_timestamp(-usec).format("s.u"));
    }
    assert_text(
        &out,
        r#"plus:
0.999_999_0: 00.999999
0.999_999_1: 00.999999
0.999_999_8: 01.000000
0.999_999_9: 01.000000
1.000_000_0: 01.000000
1.000_000_1: 01.000000
1.000_000_8: 01.000001
1.000_000_9: 01.000001
1.000_001_0: 01.000001
1.000_001_1: 01.000001
1.000_001_8: 01.000002
1.000_001_9: 01.000002

minus:
-0.999_999_0: 59.000001
-0.999_999_1: 59.000001
-0.999_999_8: 59.000000
-0.999_999_9: 59.000000
-1.000_000_0: 59.000000
-1.000_000_1: 59.000000
-1.000_000_8: 58.999999
-1.000_000_9: 58.999999
-1.000_001_0: 58.999999
-1.000_001_1: 58.999999
-1.000_001_8: 58.999998
-1.000_001_9: 58.999998"#,
    );
}

/// ext/date/tests/gh18481.phpt (with the fixed NOW: the result is false for any time)
#[test]
fn gh18481() {
    for offset in [-f64::NAN, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let r = php_timelib::sunrise_sunset(
            NOW,
            SunFormat::String,
            38.4,
            -9.0,
            90.0,
            Some(offset),
            &tz("UTC"),
            false,
        );
        assert_eq!(r, None, "offset {offset}");
    }
}

/// ext/date/tests/gh9700.phpt
#[test]
fn gh9700() {
    let d = DT::from_format(
        "Y-m-d\\TH:i:sP[e]",
        "2022-02-18T00:00:00+01:00[Europe/Berlin]",
        None,
        "UTC",
    )
    .unwrap();
    assert_expectf(
        &d.var_dump("DateTime", 1),
        r#"object(DateTime)#%d (%d) {
  ["date"]=>
  string(26) "2022-02-18 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/Berlin"
}"#,
    );
}

/// ext/date/tests/gmdate_variation11.phpt (with the fixed NOW for `gmdate()` without a
/// timestamp, which the EXPECTF does not check)
#[test]
fn gmdate_variation11() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    for (title, f) in [
        ("ISO 8601 date format", "c"),
        ("RFC 2822 date format", "r"),
        ("seconds since Unix Epoch format", "U"),
    ] {
        out += &format!("\n-- Testing gmdate() function with {title} --\n");
        out += &vd_str(&gmdate(f, NOW));
        out += &vd_str(&gmdate(f, timestamp));
    }
    assert_expectf(
        &out,
        r#"*** Testing gmdate() : usage variation ***

-- Testing gmdate() function with ISO 8601 date format --
string(%d) "%s"
string(25) "2008-08-08T08:08:08+00:00"

-- Testing gmdate() function with RFC 2822 date format --
string(%d) "%s"
string(31) "Fri, 08 Aug 2008 08:08:08 +0000"

-- Testing gmdate() function with seconds since Unix Epoch format --
string(%d) "%d"
string(10) "1218182888""#,
    );
}

/// ext/date/tests/gmdate_variation5.phpt (with the fixed NOW for `gmdate('W')`)
#[test]
fn gmdate_variation5() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    out += "\n-- Testing gmdate() function with ISO-8601 week number of year format --\n";
    out += &vd_str(&gmdate("W", NOW));
    out += &vd_str(&gmdate("W", timestamp));
    assert_expectf(
        &out,
        r#"*** Testing gmdate() : usage variation ***

-- Testing gmdate() function with ISO-8601 week number of year format --
string(%d) "%d"
string(2) "32""#,
    );
}

/// ext/date/tests/gmmktime_variation8.phpt
#[test]
fn gmmktime_variation8() {
    let mut out = String::from("*** Testing gmmktime() : usage variation ***\n");
    out += "\n-- Testing gmmktime() function with supplying octal values to arguments --\n";
    out += &V::Int(gmmktime(0o10, 0o10, 0o10, 0o10, 0o10, 0o3730)).var_dump();
    out += "\n-- Testing gmmktime() function with supplying hexa decimal values to arguments --\n";
    out += &V::Int(gmmktime(0x8, 0x8, 0x8, 0x8, 0x8, 0x7D8)).var_dump();
    assert_text(
        &out,
        r#"*** Testing gmmktime() : usage variation ***

-- Testing gmmktime() function with supplying octal values to arguments --
int(1218182888)

-- Testing gmmktime() function with supplying hexa decimal values to arguments --
int(1218182888)"#,
    );
}

/// Shared by the two `oom_*modify` tests: a 4 MiB string of `\x01` fails to parse at
/// position 0 with "Unexpected character".
fn oom_modify() {
    let mut d = DT::new("now", "UTC");
    let before = d.t.clone();
    let s = vec![1u8; 4 * 1024 * 1024];
    let err = php::date_modify(&mut d.t, &s).unwrap_err();
    assert_eq!(err.errors[0].position, 0);
    assert_eq!(err.errors[0].message, "Unexpected character");
    assert_eq!(d.t.sse, before.sse);
}

/// ext/date/tests/date_default_timezone_get-4.phpt (partial: the ini value is rejected
/// because it is not a valid identifier, so the default stays UTC)
#[test]
fn date_default_timezone_get_4() {
    assert!(!php_timelib::timezone_id_is_valid("Incorrect/Zone"));
    assert!(php_timelib::timezone_id_is_valid("UTC"));
}

/// ext/date/tests/ini_set_incorrect-002.phpt (partial: which values `ini_set()`
/// accepts for date.timezone, `timelib_timezone_id_is_valid`)
#[test]
fn ini_set_incorrect_002() {
    let mut current = "UTC";
    let mut out = format!("{current}\n");
    for value in ["foo", "Europe/London", "Mars/Valles_Marineris"] {
        if php_timelib::timezone_id_is_valid(value) {
            current = value;
        }
        out += &format!("{current}\n");
    }
    assert_text(
        &out,
        r#"UTC
UTC
Europe/London
Europe/London"#,
    );
}

/// ext/date/tests/multiple_calls_date_period_iterator_current.phpt (each `current()`
/// returns a fresh copy of the period's current time)
#[test]
fn multiple_calls_date_period_iterator_current() {
    let start = DT::new("2018-12-31 00:00:00", "UTC");
    let interval = php::date_interval_create("P1M").unwrap();
    let first = period(&start.t, &interval.diff, None, 1, true, false)
        .into_iter()
        .next()
        .unwrap();
    let mut out = String::new();
    for n in 0..3 {
        let mut current = dt_of(first.clone(), "UTC");
        out += &current.var_dump("DateTime", 1);
        if n == 1 {
            // $iter->current()->setTimestamp(0) only changes that copy.
            current.set_timestamp(0);
        }
    }
    assert_expectf(
        &out,
        r#"object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2018-12-31 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2018-12-31 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2018-12-31 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#,
    );
}

/// ext/date/tests/oom_date_modify.phpt
#[test]
fn oom_date_modify() {
    oom_modify();
}

/// ext/date/tests/oom_datetime_modify.phpt
#[test]
fn oom_datetime_modify() {
    oom_modify();
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type3-ba.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type3_ba() {
    let date_format = "Y-m-d H:i:s T e";
    let mut out = String::new();
    for (n, (start, spec)) in [
        ("2010-11-07 01:59:59", "PT1S"),
        ("2010-11-06 04:30:00", "P1D"),
        ("2010-11-06 04:30:00", "PT24H"),
        ("2010-11-06 04:30:00", "PT23H"),
        ("2010-11-06 04:30:00", "PT22H"),
        ("2010-11-06 04:30:00", "PT21H"),
        ("2010-11-06 01:30:00", "P1D"),
        ("2010-11-06 01:30:00", "P1DT1H"),
        ("2010-11-06 04:30:00", "PT25H"),
        ("2010-11-06 03:30:00", "P1D"),
        ("2010-11-06 02:30:00", "P1D"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut start = DT::new(start, NY);
        let before = start.format(date_format);
        start.add(spec);
        out += &format!(
            "ba{} {before} + {spec} = {}\n",
            n + 1,
            start.format(date_format)
        );
    }
    assert_text(
        &out,
        r#"ba1 2010-11-07 01:59:59 EDT America/New_York + PT1S = 2010-11-07 01:00:00 EST America/New_York
ba2 2010-11-06 04:30:00 EDT America/New_York + P1D = 2010-11-07 04:30:00 EST America/New_York
ba3 2010-11-06 04:30:00 EDT America/New_York + PT24H = 2010-11-07 03:30:00 EST America/New_York
ba4 2010-11-06 04:30:00 EDT America/New_York + PT23H = 2010-11-07 02:30:00 EST America/New_York
ba5 2010-11-06 04:30:00 EDT America/New_York + PT22H = 2010-11-07 01:30:00 EST America/New_York
ba6 2010-11-06 04:30:00 EDT America/New_York + PT21H = 2010-11-07 01:30:00 EDT America/New_York
ba7 2010-11-06 01:30:00 EDT America/New_York + P1D = 2010-11-07 01:30:00 EDT America/New_York
ba8 2010-11-06 01:30:00 EDT America/New_York + P1DT1H = 2010-11-07 01:30:00 EST America/New_York
ba9 2010-11-06 04:30:00 EDT America/New_York + PT25H = 2010-11-07 04:30:00 EST America/New_York
ba10 2010-11-06 03:30:00 EDT America/New_York + P1D = 2010-11-07 03:30:00 EST America/New_York
ba11 2010-11-06 02:30:00 EDT America/New_York + P1D = 2010-11-07 02:30:00 EST America/New_York"#,
    );
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type3-fs.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type3_fs() {
    let date_format = "Y-m-d H:i:s T e";
    let mut out = String::new();
    for (n, (end, spec)) in [
        ("2010-03-14 03:00:00", "PT1S"),
        ("2010-03-14 04:30:00", "P1D"),
        ("2010-03-14 03:30:00", "PT22H"),
        ("2010-03-14 01:30:00", "PT21H"),
        ("2010-03-14 01:30:00", "P1D"),
        ("2010-03-15 03:30:00", "P1D"),
        ("2010-03-15 02:30:00", "P1D"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut end = DT::new(end, NY);
        let before = end.format(date_format);
        end.sub(spec);
        out += &format!(
            "fs{} {before} - {spec} = {}\n",
            n + 1,
            end.format(date_format)
        );
    }
    assert_text(
        &out,
        r#"fs1 2010-03-14 03:00:00 EDT America/New_York - PT1S = 2010-03-14 01:59:59 EST America/New_York
fs2 2010-03-14 04:30:00 EDT America/New_York - P1D = 2010-03-13 04:30:00 EST America/New_York
fs3 2010-03-14 03:30:00 EDT America/New_York - PT22H = 2010-03-13 04:30:00 EST America/New_York
fs4 2010-03-14 01:30:00 EST America/New_York - PT21H = 2010-03-13 04:30:00 EST America/New_York
fs5 2010-03-14 01:30:00 EST America/New_York - P1D = 2010-03-13 01:30:00 EST America/New_York
fs6 2010-03-15 03:30:00 EDT America/New_York - P1D = 2010-03-14 03:30:00 EDT America/New_York
fs7 2010-03-15 02:30:00 EDT America/New_York - P1D = 2010-03-14 03:30:00 EDT America/New_York"#,
    );
}

/// ext/date/tests/strtotime2.phpt (with the fixed NOW instead of time())
#[test]
fn strtotime2() {
    let z = "Europe/Oslo";
    let mut out = String::new();
    for (name, f) in [
        ("DATE_ATOM", DATE_ATOM),
        ("DATE_COOKIE", DATE_COOKIE),
        ("DATE_ISO8601", DATE_ISO8601),
        ("DATE_ISO8601_EXPANDED", DATE_ISO8601_EXPANDED),
        ("DATE_RFC822", DATE_RFC822),
        ("DATE_RFC850", DATE_RFC850),
        ("DATE_RFC1036", DATE_RFC1036),
        ("DATE_RFC1123", DATE_RFC1123),
        ("DATE_RFC2822", DATE_RFC2822),
        ("DATE_RFC3339", DATE_RFC3339),
        ("DATE_RFC3339_EXTENDED", DATE_RFC3339_EXTENDED),
        ("DATE_RSS", DATE_RSS),
        ("DATE_W3C", DATE_W3C),
    ] {
        let ok = strtotime(&date(f, NOW, z), NOW, z) == Some(NOW);
        out += &format!("{name}:\t{}\n", if ok { "OK" } else { "FAIL" });
    }
    assert_text(
        &out,
        r#"DATE_ATOM:	OK
DATE_COOKIE:	OK
DATE_ISO8601:	OK
DATE_ISO8601_EXPANDED:	OK
DATE_RFC822:	OK
DATE_RFC850:	OK
DATE_RFC1036:	OK
DATE_RFC1123:	OK
DATE_RFC2822:	OK
DATE_RFC3339:	OK
DATE_RFC3339_EXTENDED:	OK
DATE_RSS:	OK
DATE_W3C:	OK"#,
    );
}

/// ext/date/tests/sunfuncts.phpt
#[test]
fn sunfuncts() {
    let zone = tz("Asia/Jerusalem");
    let show = |r: Option<SunResult>| match r {
        Some(SunResult::Timestamp(t)) => t.to_string(),
        Some(SunResult::String(s)) => s,
        Some(SunResult::Double(d)) => php_float_prec(d, 13),
        None => String::new(),
    };
    let mut out = String::new();
    for a in 1..=12 {
        let ts = mktime(1, 1, 1, a, 1, 2003, "Asia/Jerusalem");
        for sunset in [false, true] {
            let line: Vec<String> = [SunFormat::Timestamp, SunFormat::String, SunFormat::Double]
                .into_iter()
                .map(|f| {
                    show(php_timelib::sunrise_sunset(
                        ts,
                        f,
                        31.76670,
                        35.23330,
                        90.83,
                        Some(2.0),
                        &zone,
                        sunset,
                    ))
                })
                .collect();
            out += &format!("{}\n", line.join(" "));
        }
    }
    assert_expectf(
        &out,
        r#"1041395858 06:37 6.6274307083%d
1041432434 16:47 16.787476435%d
1044073871 06:31 6.5198435465%d
1044112440 17:14 17.233560530%d
1046491524 06:05 6.0901858662%d
1046533056 17:37 17.626724279%d
1049167610 05:26 5.4473483216%d
1049212759 17:59 17.988854003%d
1051757554 04:52 4.8761196793%d
1051805991 18:19 18.330987470%d
1054434782 04:33 4.5506171719%d
1054485634 18:40 18.676282498%d
1057026940 04:35 4.5946862151%d
1057078197 18:49 18.832651294%d
1059706395 04:53 4.8875657219%d
1059755854 18:37 18.626282855%d
1062385985 05:13 5.2182752537%d
1062432319 18:05 18.088728929%d
1064979083 05:31 5.5233314120%d
1065021981 17:26 17.439368589%d
1067658825 05:53 5.8959513887%d
1067698295 16:51 16.859972460%d
1070252367 06:19 6.3241991396%d
1070289384 16:36 16.606870705%d"#,
    );
}

/// ext/date/tests/timezone_abbreviations_list_basic1.phpt
#[test]
fn timezone_abbreviations_list_basic1() {
    let abbr = timezone_abbreviations_list();
    let mut out =
        String::from("*** Testing timezone_abbreviations_list() : basic functionality ***\n");
    out += &vd_str("array");
    out += &V::Int(abbr.len() as i64).var_dump();
    out += "\n-- Format a sample entry --\n";
    out += &abbr
        .iter()
        .find(|(k, _)| *k == K::S("acst".into()))
        .unwrap()
        .1
        .var_dump();
    assert_text(
        &out,
        r#"*** Testing timezone_abbreviations_list() : basic functionality ***
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
}"#,
    );
}

/// ext/date/tests/timezones-list.phpt
#[test]
fn timezones_list() {
    let a = timezone_identifiers_list(TZ_ALL);
    let b = timezone_identifiers_list(0x0002);
    let c = timezone_identifiers_list(0x0FFF);
    let d = timezone_identifiers_list(0x0080 | 0x0400);
    let mut out = String::new();
    for id in ["Europe/Oslo", "America/New_York", "UTC", "US/Eastern"] {
        for list in [&a, &b, &c, &d] {
            let found = list.iter().any(|x| x == id);
            out += if found { "found\n" } else { "notfound\n" };
        }
        out += "\n";
    }
    assert_text(
        &out,
        r#"found
notfound
found
found

found
found
found
notfound

found
notfound
found
found

notfound
notfound
found
notfound"#,
    );
}
