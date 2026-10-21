//! Tests ported from `ext/date/tests/*.phpt` (group 2).
mod common;
use common::*;

use std::fmt::Write as _;

use php_timelib::php::{self, PhpInterval, PhpTimeZone};
use php_timelib::{RelTime, SunFormat, SunValue, Time, builtin_db, builtin_tzinfo};

const DATE_ISO8601: &str = "Y-m-d\\TH:i:sO";
const DATE_RFC3339_EXTENDED: &str = "Y-m-d\\TH:i:s.vP";

// ---------------------------------------------------------------------------------
// Local helpers.

/// Matches one line against a phpt EXPECTF line (`%s`, `%d`, `%i`, `%a`).
fn expectf_line(actual: &[u8], pattern: &[u8]) -> bool {
    let (mut a, mut p) = (0, 0);
    while p < pattern.len() {
        if pattern[p] == b'%' && p + 1 < pattern.len() {
            let class = pattern[p + 1];
            let ok = |c: u8| match class {
                b's' => c != b'\n' && c != b'\r',
                b'd' => c.is_ascii_digit(),
                b'i' => c.is_ascii_digit() || c == b'+' || c == b'-',
                b'a' => true,
                _ => false,
            };
            if !matches!(class, b's' | b'd' | b'i' | b'a') {
                if a < actual.len() && actual[a] == b'%' {
                    a += 1;
                    p += 1;
                    continue;
                }
                return false;
            }
            let mut n = a;
            while n < actual.len() && ok(actual[n]) {
                n += 1;
                if expectf_line(&actual[n..], &pattern[p + 2..]) {
                    return true;
                }
            }
            return false;
        }
        if a >= actual.len() || actual[a] != pattern[p] {
            return false;
        }
        a += 1;
        p += 1;
    }
    a == actual.len()
}

/// Asserts that `actual` matches an EXPECTF pattern, line by line.
#[track_caller]
fn assert_textf(actual: &str, pattern: &str) {
    let a: Vec<&str> = actual.trim_end().lines().collect();
    let p: Vec<&str> = pattern.trim_end().lines().collect();
    for (i, (la, lp)) in a.iter().zip(p.iter()).enumerate() {
        if !expectf_line(la.as_bytes(), lp.as_bytes()) {
            panic!(
                "EXPECTF mismatch at line {}:\n  actual:  {la:?}\n  pattern: {lp:?}\n--- actual ---\n{actual}",
                i + 1
            );
        }
    }
    assert_eq!(
        a.len(),
        p.len(),
        "line count differs\n--- actual ---\n{actual}"
    );
}

/// Guards the EXPECTF matcher itself.
#[test]
fn expectf_matcher_self_check() {
    assert!(expectf_line(b"int(42)", b"int(%d)"));
    assert!(!expectf_line(b"int()", b"int(%d)"));
    assert!(!expectf_line(b"int(4x)", b"int(%d)"));
    assert!(expectf_line(
        b"ts = Monday 17:17:17 +04",
        b"ts = Monday 17:17:17 %s"
    ));
    assert!(!expectf_line(
        b"ts = Monday 17:17:17 ",
        b"ts = Monday 17:17:17 %s"
    ));
    assert!(!expectf_line(b"abc", b"abd"));
    assert!(!expectf_line(b"abcd", b"abc"));
}

/// `var_dump($string)`.
fn vd_str(s: &str) -> String {
    V::Str(s.to_string()).var_dump()
}

/// `var_dump($int)`.
fn vd_int(i: i64) -> String {
    V::Int(i).var_dump()
}

/// `date_create($s)`: `None` where PHP returns `false`.
fn dt(s: &str, zone: &str) -> Option<DT> {
    DT::try_new(s, None, zone).ok()
}

/// `var_dump(new DateTimeZone(...))`.
fn tz_var_dump(z: &PhpTimeZone, id: u32) -> String {
    V::Obj(
        "DateTimeZone".into(),
        id,
        vec![
            (
                K::S("timezone_type".into()),
                V::Int(i64::from(z.zone_type().as_i32())),
            ),
            (K::S("timezone".into()), V::Str(z.name())),
        ],
    )
    .var_dump()
}

/// `$a <=> $b` for two `DateTime` objects (`timelib_time_compare`).
fn gt(a: &DT, b: &DT) -> bool {
    a.t.compare(&b.t) == std::cmp::Ordering::Greater
}

/// One step of a `DatePeriod` iteration (`date_period_advance`).
fn period_advance(t: &mut Time, interval: &RelTime) {
    t.have_relative = true;
    t.relative = *interval;
    t.sse_uptodate = false;
    t.update_ts(None);
    t.update_from_sse();
}

/// `DateTime::format()` of a raw time (as the DatePeriod iterator's objects do).
fn fmt_time(t: &Time, f: &str) -> String {
    String::from_utf8(php_timelib::date_format(f.as_bytes(), t, t.is_localtime)).unwrap()
}

/// The `$interval->f = $f` property write.
fn set_f(iv: &mut PhpInterval, f: f64) {
    iv.diff.us = (f * 1_000_000.0) as i64;
}

// ---------------------------------------------------------------------------------
// examine_diff.inc

#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Diff,
    Days,
    Add,
    Sub,
}

struct Case {
    name: &'static str,
    end: &'static str,
    end_tz: bool,
    start: &'static str,
    start_tz: bool,
    spec: &'static str,
}

/// `examine_diff()` from `examine_diff.inc` (default zone America/New_York).
fn examine_diff(cases: &[Case], show: Show) -> String {
    let zone = "America/New_York";
    let mut out = String::new();
    for c in cases {
        let mut start = DT::new(c.start, zone);
        if c.start_tz {
            start.set_timezone(zone);
        }
        let start_date = start.format("Y-m-d H:i:s T");
        let mut end = DT::new(c.end, zone);
        if c.end_tz {
            end.set_timezone(zone);
        }
        let end_date = end.format("Y-m-d H:i:s T");
        let mut expect_interval = php::date_interval_create(&format!("P{}", &c.spec[2..])).unwrap();
        if &c.spec[1..2] == "-" {
            expect_interval.diff.invert = true;
        }
        let line = match show {
            Show::Diff => {
                let r = start.diff(&end);
                let spec = interval_format(&r, "P%R%yY%mM%dDT%hH%iM%sS");
                format!("DIFF: {end_date} - {start_date} = **{spec}**")
            }
            Show::Days => {
                let r = start.diff(&end);
                format!("DAYS: **{}**", interval_format(&r, "%a"))
            }
            Show::Add => {
                start.add_interval(&expect_interval);
                let r = start.format("Y-m-d H:i:s T");
                format!("ADD: {start_date} + {} = **{r}**", c.spec)
            }
            Show::Sub => {
                end.sub_interval(&expect_interval);
                let r = end.format("Y-m-d H:i:s T");
                format!("SUB: {end_date} - {} = **{r}**", c.spec)
            }
        };
        let _ = writeln!(out, "{}: {line}", c.name);
    }
    out
}

// ---------------------------------------------------------------------------------
// The tests.

/// ext/date/tests/004.phpt
#[test]
fn t004() {
    let t = mktime(0, 0, 0, 6, 27, 2006, "UTC");
    let mut out = String::new();
    for zone in ["UTC", "Asia/Jerusalem", "America/Chicago", "Europe/London"] {
        for f in ["w", "z", "n", "t", "L", "a", "B", "g", "G", "Z", "U"] {
            out.push_str(&vd_str(&date(f, t, zone)));
        }
    }
    out.push_str("Done\n");
    assert_text(
        &out,
        r##"string(1) "2"
string(3) "177"
string(1) "6"
string(2) "30"
string(1) "0"
string(2) "am"
string(3) "041"
string(2) "12"
string(1) "0"
string(1) "0"
string(10) "1151366400"
string(1) "2"
string(3) "177"
string(1) "6"
string(2) "30"
string(1) "0"
string(2) "am"
string(3) "041"
string(1) "3"
string(1) "3"
string(5) "10800"
string(10) "1151366400"
string(1) "1"
string(3) "176"
string(1) "6"
string(2) "30"
string(1) "0"
string(2) "pm"
string(3) "041"
string(1) "7"
string(2) "19"
string(6) "-18000"
string(10) "1151366400"
string(1) "2"
string(3) "177"
string(1) "6"
string(2) "30"
string(1) "0"
string(2) "am"
string(3) "041"
string(1) "1"
string(1) "1"
string(4) "3600"
string(10) "1151366400"
Done"##,
    );
}

/// ext/date/tests/015.phpt
#[test]
fn t015() {
    let mut dto = DT::new("now", "UTC");
    let tzold = PhpTimeZone::from_time(&dto.t).unwrap();
    let mut out = vd_str(&tzold.name());
    dto.set_timezone("US/Eastern");
    out.push_str(&vd_str(&tzold.name()));
    out.push_str(&vd_str(&dto.timezone_name()));
    out.push_str(&vd_str(&tzold.name()));
    out.push_str("Done\n");
    assert_text(
        &out,
        r##"string(3) "UTC"
string(3) "UTC"
string(10) "US/Eastern"
string(3) "UTC"
Done"##,
    );
}

/// ext/date/tests/DateInterval_format_a.phpt
#[test]
fn dateinterval_format_a() {
    let date1 = DT::new("2000-01-01 00:00:00", "UTC");
    let date2 = DT::new("2001-03-04 04:05:06", "UTC");
    let interval = date1.diff(&date2);
    assert_eq!(interval_format(&interval, "a=%a"), "a=428");
}

/// ext/date/tests/DateTimeImmutable_createFromMutable-001.phpt (partial: the TypeError
/// is not expressible)
#[test]
fn datetimeimmutable_createfrommutable_001() {
    let i = DT::new("2014-03-02 16:24:08", "Europe/London");
    assert_textf(
        &i.var_dump("DateTimeImmutable", 1),
        r#"object(DateTimeImmutable)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}"#,
    );
}

/// ext/date/tests/DateTimeZone_clone_basic1.phpt (partial: the clone is a plain copy)
#[test]
fn datetimezone_clone_basic1() {
    let orig = php::timezone_open("GMT").unwrap();
    let clone = orig.clone();
    let mut out = String::from("*** Testing clone on DateTime objects ***\n");
    out.push_str(&tz_var_dump(&orig, 1));
    out.push_str(&tz_var_dump(&clone, 2));
    assert_eq!(clone, orig);
    out.push_str("TEST PASSED : Objects equal but not identical\n");
    assert_textf(
        &out,
        r##"*** Testing clone on DateTime objects ***
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "GMT"
}
object(DateTimeZone)#%d (2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "GMT"
}
TEST PASSED : Objects equal but not identical"##,
    );
}

/// ext/date/tests/DateTimeZone_construct_basic.phpt
#[test]
fn datetimezone_construct_basic() {
    let mut out = String::from("*** Testing new DateTimeZone() : basic functionality ***\n");
    for (i, name) in ["GMT", "Europe/London", "America/Los_Angeles"]
        .iter()
        .enumerate()
    {
        out.push_str(&tz_var_dump(
            &php::timezone_open(name).unwrap(),
            i as u32 + 1,
        ));
    }
    assert_textf(
        &out,
        r##"*** Testing new DateTimeZone() : basic functionality ***
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
}"##,
    );
}

/// ext/date/tests/DateTimeZone_getTransitions_basic1.phpt
#[test]
fn datetimezone_gettransitions_basic1() {
    let tzi = tz("Europe/London");
    let tran = php::timezone_transitions_get(&tzi, -306_972_000, -37_241_999);
    let mut out =
        String::from("*** Testing DateTimeZone::getTransitions() : basic functionality ***\n");
    let _ = writeln!(out, "\n-- Total number of transitions: {} --", tran.len());
    out.push_str("\n-- Format a sample entry for Spring 1963 --\n");
    let t = &tran[6];
    let v = V::Arr(vec![
        (K::S("ts".into()), V::Int(t.ts)),
        (K::S("time".into()), V::Str(t.time())),
        (K::S("offset".into()), V::Int(i64::from(t.offset))),
        (K::S("isdst".into()), V::Bool(t.isdst)),
        (K::S("abbr".into()), V::Str(t.abbr.clone())),
    ]);
    out.push_str(&v.var_dump());
    assert_text(
        &out,
        r##"*** Testing DateTimeZone::getTransitions() : basic functionality ***

-- Total number of transitions: 18 --

-- Format a sample entry for Spring 1963 --
array(5) {
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
}"##,
    );
}

/// ext/date/tests/DateTime_add-february.phpt
#[test]
fn datetime_add_february() {
    assert_text(
        &examine_diff(FEBRUARY, Show::Add),
        r##"test_bug_49081__1: ADD: 2010-03-01 00:00:00 EST + P+0Y0M30DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081__2: ADD: 2010-03-01 00:00:00 EST + P+0Y1M0DT0H0M0S = **2010-04-01 00:00:00 EDT**
test_bug_49081__3: ADD: 2010-03-31 00:00:00 EDT + P+0Y0M1DT0H0M0S = **2010-04-01 00:00:00 EDT**
test_bug_49081__4: ADD: 2010-03-31 00:00:00 EDT + P+0Y0M29DT0H0M0S = **2010-04-29 00:00:00 EDT**
test_bug_49081__5: ADD: 2010-03-31 00:00:00 EDT + P+0Y0M30DT0H0M0S = **2010-04-30 00:00:00 EDT**
test_bug_49081__6: ADD: 2010-03-30 00:00:00 EDT + P+0Y1M0DT0H0M0S = **2010-04-30 00:00:00 EDT**
test_bug_49081__7: ADD: 2010-03-29 00:00:00 EDT + P+0Y1M1DT0H0M0S = **2010-04-30 00:00:00 EDT**
test_bug_49081__8: ADD: 2010-01-01 00:00:00 EST + P+0Y0M28DT0H0M0S = **2010-01-29 00:00:00 EST**
test_bug_49081__9: ADD: 2010-01-01 00:00:00 EST + P+0Y0M29DT0H0M0S = **2010-01-30 00:00:00 EST**
test_bug_49081__10: ADD: 2010-01-01 00:00:00 EST + P+0Y0M30DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081__11: ADD: 2010-01-01 00:00:00 EST + P+0Y1M0DT0H0M0S = **2010-02-01 00:00:00 EST**
test_bug_49081__12: ADD: 2010-01-31 00:00:00 EST + P+0Y0M1DT0H0M0S = **2010-02-01 00:00:00 EST**
test_bug_49081__13: ADD: 2010-01-31 00:00:00 EST + P+0Y0M27DT0H0M0S = **2010-02-27 00:00:00 EST**
test_bug_49081__14: ADD: 2010-01-31 00:00:00 EST + P+0Y0M28DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__15: ADD: 2010-01-30 00:00:00 EST + P+0Y0M29DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__16: ADD: 2010-01-29 00:00:00 EST + P+0Y0M30DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__17: ADD: 2010-01-28 00:00:00 EST + P+0Y1M0DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__18: ADD: 2010-01-27 00:00:00 EST + P+0Y1M1DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081__19: ADD: 2010-01-01 00:00:00 EST + P+0Y2M0DT0H0M0S = **2010-03-01 00:00:00 EST**
test_bug_49081__20: ADD: 2010-01-31 00:00:00 EST + P+0Y0M29DT0H0M0S = **2010-03-01 00:00:00 EST**
test_bug_49081__21: ADD: 2010-01-31 00:00:00 EST + P+0Y1M24DT0H0M0S = **2010-03-27 00:00:00 EDT**
test_bug_49081__22: ADD: 2010-01-31 00:00:00 EST + P+0Y1M25DT0H0M0S = **2010-03-28 00:00:00 EDT**
test_bug_49081__23: ADD: 2010-01-31 00:00:00 EST + P+0Y1M26DT0H0M0S = **2010-03-29 00:00:00 EDT**
test_bug_49081__24: ADD: 2010-01-31 00:00:00 EST + P+0Y1M27DT0H0M0S = **2010-03-30 00:00:00 EDT**
test_bug_49081__25: ADD: 2010-01-31 00:00:00 EST + P+0Y2M0DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081__26: ADD: 2010-01-30 00:00:00 EST + P+0Y2M1DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081__27: ADD: 2009-01-01 00:00:00 EST + P+0Y0M30DT0H0M0S = **2009-01-31 00:00:00 EST**
test_bug_49081__28: ADD: 2010-02-28 00:00:00 EST + P+0Y0M27DT0H0M0S = **2010-03-27 00:00:00 EDT**
test_bug_49081__29: ADD: 2010-02-28 00:00:00 EST + P+0Y1M0DT0H0M0S = **2010-03-28 00:00:00 EDT**
test_bug_49081__30: ADD: 2010-02-28 00:00:00 EST + P+0Y1M1DT0H0M0S = **2010-03-29 00:00:00 EDT**
test_bug_49081__31: ADD: 2010-02-27 00:00:00 EST + P+0Y1M0DT0H0M0S = **2010-03-27 00:00:00 EDT**
test_bug_49081__32: ADD: 2010-02-26 00:00:00 EST + P+0Y1M1DT0H0M0S = **2010-03-27 00:00:00 EDT**
test_bug_49081_negative__1: ADD: 2010-03-31 00:00:00 EDT + P-0Y0M30DT0H0M0S = **2010-03-01 00:00:00 EST**
test_bug_49081_negative__2: ADD: 2010-04-01 00:00:00 EDT + P-0Y1M0DT0H0M0S = **2010-03-01 00:00:00 EST**
test_bug_49081_negative__3: ADD: 2010-04-01 00:00:00 EDT + P-0Y0M1DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081_negative__4: ADD: 2010-04-29 00:00:00 EDT + P-0Y0M29DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081_negative__5: ADD: 2010-04-30 00:00:00 EDT + P-0Y0M30DT0H0M0S = **2010-03-31 00:00:00 EDT**
test_bug_49081_negative__6: ADD: 2010-04-30 00:00:00 EDT + P-0Y1M0DT0H0M0S = **2010-03-30 00:00:00 EDT**
test_bug_49081_negative__7: ADD: 2010-04-30 00:00:00 EDT + P-0Y1M1DT0H0M0S = **2010-03-29 00:00:00 EDT**
test_bug_49081_negative__8: ADD: 2010-01-29 00:00:00 EST + P-0Y0M28DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081_negative__9: ADD: 2010-01-30 00:00:00 EST + P-0Y0M29DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081_negative__10: ADD: 2010-01-31 00:00:00 EST + P-0Y0M30DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081_negative__11: ADD: 2010-02-01 00:00:00 EST + P-0Y1M0DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081_negative__12: ADD: 2010-02-01 00:00:00 EST + P-0Y0M1DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__13: ADD: 2010-02-27 00:00:00 EST + P-0Y0M27DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__14: ADD: 2010-02-28 00:00:00 EST + P-0Y0M28DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__15: ADD: 2010-02-28 00:00:00 EST + P-0Y0M29DT0H0M0S = **2010-01-30 00:00:00 EST**
test_bug_49081_negative__16: ADD: 2010-02-28 00:00:00 EST + P-0Y0M30DT0H0M0S = **2010-01-29 00:00:00 EST**
test_bug_49081_negative__17: ADD: 2010-02-28 00:00:00 EST + P-0Y1M0DT0H0M0S = **2010-01-28 00:00:00 EST**
test_bug_49081_negative__18: ADD: 2010-02-28 00:00:00 EST + P-0Y1M1DT0H0M0S = **2010-01-27 00:00:00 EST**
test_bug_49081_negative__19: ADD: 2010-03-01 00:00:00 EST + P-0Y2M0DT0H0M0S = **2010-01-01 00:00:00 EST**
test_bug_49081_negative__20: ADD: 2010-03-01 00:00:00 EST + P-0Y1M1DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__21: ADD: 2010-03-27 00:00:00 EDT + P-0Y1M27DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__22: ADD: 2010-03-28 00:00:00 EDT + P-0Y1M28DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__23: ADD: 2010-03-29 00:00:00 EDT + P-0Y1M29DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__24: ADD: 2010-03-30 00:00:00 EDT + P-0Y1M30DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__25: ADD: 2010-03-31 00:00:00 EDT + P-0Y2M0DT0H0M0S = **2010-01-31 00:00:00 EST**
test_bug_49081_negative__26: ADD: 2010-03-31 00:00:00 EDT + P-0Y2M1DT0H0M0S = **2010-01-30 00:00:00 EST**
test_bug_49081_negative__27: ADD: 2009-01-31 00:00:00 EST + P-0Y0M30DT0H0M0S = **2009-01-01 00:00:00 EST**
test_bug_49081_negative__28: ADD: 2010-03-27 00:00:00 EDT + P-0Y0M27DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__29: ADD: 2010-03-28 00:00:00 EDT + P-0Y1M0DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__30: ADD: 2010-03-29 00:00:00 EDT + P-0Y1M1DT0H0M0S = **2010-02-28 00:00:00 EST**
test_bug_49081_negative__31: ADD: 2010-03-27 00:00:00 EDT + P-0Y1M0DT0H0M0S = **2010-02-27 00:00:00 EST**
test_bug_49081_negative__32: ADD: 2010-03-27 00:00:00 EDT + P-0Y1M1DT0H0M0S = **2010-02-26 00:00:00 EST**"##,
    );
}

/// ext/date/tests/DateTime_clone_basic1.phpt (partial: the clone is a plain copy)
#[test]
fn datetime_clone_basic1() {
    let mut orig = DT::new("2008-07-02 14:25:41", "Europe/London");
    let clone = orig.clone();
    orig.set_time(22, 41, 50, 0);
    let out = format!(
        "*** Testing clone on DateTime objects ***\nOriginal: {}\nClone: {}\n",
        orig.format("H:i:s"),
        clone.format("H:i:s")
    );
    assert_text(
        &out,
        r##"*** Testing clone on DateTime objects ***
Original: 22:41:50
Clone: 14:25:41"##,
    );
}

/// ext/date/tests/DateTime_createFromInterface-001.phpt
#[test]
fn datetime_createfrominterface_001() {
    let mut out = String::new();
    for current in [
        "2014-03-02 16:24:08",
        "2019-12-16 15:06:46 CET",
        "2019-12-16 15:08:20 +0100",
    ] {
        // date_create() and date_create_immutable() followed by a copy.
        for _ in 0..2 {
            out.push_str(&DT::new(current, "Europe/London").var_dump("DateTime", 1));
        }
    }
    assert_textf(
        &out,
        r##"object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:06:46.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "CET"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2019-12-16 15:08:20.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}"##,
    );
}

/// ext/date/tests/DateTime_days-fall-type3-type2.phpt
#[test]
fn datetime_days_fall_type3_type2() {
    assert_text(
        &examine_diff(FALL_TYPE3_TYPE2, Show::Days),
        r##"test_time_fall_type3_prev_type2_prev: DAYS: **33**
test_time_fall_type3_prev_type2_dt: DAYS: **0**
test_time_fall_type3_prev_type2_redodt: DAYS: **0**
test_time_fall_type3_prev_type2_redost: DAYS: **0**
test_time_fall_type3_prev_type2_st: DAYS: **0**
test_time_fall_type3_prev_type2_post: DAYS: **2**
test_time_fall_type3_dt_type2_prev: DAYS: **0**
test_time_fall_type3_dt_type2_dt: DAYS: **0**
test_time_fall_type3_dt_type2_redodt: DAYS: **0**
test_time_fall_type3_dt_type2_redost: DAYS: **0**
test_time_fall_type3_dt_type2_st: DAYS: **0**
test_time_fall_type3_dt_type2_post: DAYS: **1**
test_time_fall_type3_redodt_type2_prev: DAYS: **0**
test_time_fall_type3_redodt_type2_dt: DAYS: **0**
test_time_fall_type3_redodt_type2_redodt: DAYS: **0**
test_time_fall_type3_redodt_type2_redost: DAYS: **0**
test_time_fall_type3_redodt_type2_st: DAYS: **0**
test_time_fall_type3_redodt_type2_post: DAYS: **1**
test_time_fall_type3_redost_type2_prev: DAYS: **0**
test_time_fall_type3_redost_type2_dt: DAYS: **0**
test_time_fall_type3_redost_type2_redodt: DAYS: **0**
test_time_fall_type3_redost_type2_redost: DAYS: **0**
test_time_fall_type3_redost_type2_st: DAYS: **0**
test_time_fall_type3_redost_type2_post: DAYS: **1**
test_time_fall_type3_st_type2_prev: DAYS: **0**
test_time_fall_type3_st_type2_dt: DAYS: **0**
test_time_fall_type3_st_type2_redodt: DAYS: **0**
test_time_fall_type3_st_type2_redost: DAYS: **0**
test_time_fall_type3_st_type2_st: DAYS: **0**
test_time_fall_type3_st_type2_post: DAYS: **1**
test_time_fall_type3_post_type2_prev: DAYS: **2**
test_time_fall_type3_post_type2_dt: DAYS: **1**
test_time_fall_type3_post_type2_redodt: DAYS: **1**
test_time_fall_type3_post_type2_redost: DAYS: **1**
test_time_fall_type3_post_type2_st: DAYS: **1**
test_time_fall_type3_post_type2_post: DAYS: **0**
test_time_fall_type3_dtsec_type2_stsec: DAYS: **0**
test_time_fall_type3_stsec_type2_dtsec: DAYS: **0**"##,
    );
}

/// ext/date/tests/DateTime_days-spring-type3-type2.phpt
#[test]
fn datetime_days_spring_type3_type2() {
    assert_text(
        &examine_diff(SPRING_TYPE3_TYPE2, Show::Days),
        r##"test_time_spring_type3_prev_type2_prev: DAYS: **30**
test_time_spring_type3_prev_type2_st: DAYS: **0**
test_time_spring_type3_prev_type2_dt: DAYS: **0**
test_time_spring_type3_prev_type2_post: DAYS: **2**
test_time_spring_type3_st_type2_prev: DAYS: **0**
test_time_spring_type3_st_type2_st: DAYS: **0**
test_time_spring_type3_st_type2_dt: DAYS: **0**
test_time_spring_type3_st_type2_post: DAYS: **1**
test_time_spring_type3_dt_type2_prev: DAYS: **0**
test_time_spring_type3_dt_type2_st: DAYS: **0**
test_time_spring_type3_dt_type2_dt: DAYS: **0**
test_time_spring_type3_dt_type2_post: DAYS: **1**
test_time_spring_type3_post_type2_prev: DAYS: **2**
test_time_spring_type3_post_type2_st: DAYS: **1**
test_time_spring_type3_post_type2_dt: DAYS: **1**
test_time_spring_type3_post_type2_post: DAYS: **0**
test_time_spring_type3_stsec_type2_dtsec: DAYS: **0**
test_time_spring_type3_dtsec_type2_stsec: DAYS: **0**"##,
    );
}

/// ext/date/tests/DateTime_diff-fall-type3-type2.phpt
#[test]
fn datetime_diff_fall_type3_type2() {
    assert_text(
        &examine_diff(FALL_TYPE3_TYPE2, Show::Diff),
        r##"test_time_fall_type3_prev_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-10-04 02:18:48 EDT = **P+0Y1M2DT16H19M40S**
test_time_fall_type3_prev_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT5H31M52S**
test_time_fall_type3_prev_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT6H34M5S**
test_time_fall_type3_prev_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT7H36M16S**
test_time_fall_type3_prev_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT9H38M27S**
test_time_fall_type3_prev_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M2DT2H21M31S**
test_time_fall_type3_dt_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 00:10:20 EDT = **P-0Y0M0DT5H31M52S**
test_time_fall_type3_dt_type2_dt: DIFF: 2010-11-07 00:15:35 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT0H5M15S**
test_time_fall_type3_dt_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT1H2M13S**
test_time_fall_type3_dt_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT2H4M24S**
test_time_fall_type3_dt_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT4H6M35S**
test_time_fall_type3_dt_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M1DT20H49M39S**
test_time_fall_type3_redodt_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT6H34M5S**
test_time_fall_type3_redodt_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT1H2M13S**
test_time_fall_type3_redodt_type2_redodt: DIFF: 2010-11-07 01:15:35 EDT - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT0H3M2S**
test_time_fall_type3_redodt_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT1H2M11S**
test_time_fall_type3_redodt_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT3H4M22S**
test_time_fall_type3_redodt_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M1DT19H47M26S**
test_time_fall_type3_redost_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT7H36M16S**
test_time_fall_type3_redost_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT2H4M24S**
test_time_fall_type3_redost_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT1H2M11S**
test_time_fall_type3_redost_type2_redost: DIFF: 2010-11-07 01:16:54 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT0H2M10S**
test_time_fall_type3_redost_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT2H2M11S**
test_time_fall_type3_redost_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:14:44 EST = **P+0Y0M1DT18H45M15S**
test_time_fall_type3_st_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT9H38M27S**
test_time_fall_type3_st_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT4H6M35S**
test_time_fall_type3_st_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT3H4M22S**
test_time_fall_type3_st_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 03:16:55 EST = **P-0Y0M0DT2H2M11S**
test_time_fall_type3_st_type2_st: DIFF: 2010-11-07 05:19:56 EST - 2010-11-07 03:16:55 EST = **P+0Y0M0DT2H3M1S**
test_time_fall_type3_st_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 03:16:55 EST = **P+0Y0M1DT16H43M4S**
test_time_fall_type3_post_type2_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M2DT2H21M31S**
test_time_fall_type3_post_type2_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT20H49M39S**
test_time_fall_type3_post_type2_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT19H47M26S**
test_time_fall_type3_post_type2_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT18H45M15S**
test_time_fall_type3_post_type2_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT16H43M4S**
test_time_fall_type3_post_type2_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-08 18:57:55 EST = **P+0Y0M0DT1H2M4S**
test_time_fall_type3_dtsec_type2_stsec: DIFF: 2010-11-07 01:00:00 EST - 2010-11-07 01:59:59 EDT = **P+0Y0M0DT0H0M1S**
test_time_fall_type3_stsec_type2_dtsec: DIFF: 2010-11-07 01:59:59 EDT - 2010-11-07 01:00:00 EST = **P-0Y0M0DT0H0M1S**"##,
    );
}

/// ext/date/tests/DateTime_diff-spring-type3-type2.phpt
#[test]
fn datetime_diff_spring_type3_type2() {
    assert_text(
        &examine_diff(SPRING_TYPE3_TYPE2, Show::Diff),
        r##"test_time_spring_type3_prev_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-02-11 02:18:48 EST = **P+0Y1M2DT16H19M40S**
test_time_spring_type3_prev_type2_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-13 18:38:28 EST = **P+0Y0M0DT5H31M52S**
test_time_spring_type3_prev_type2_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M0DT7H38M27S**
test_time_spring_type3_prev_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M2DT0H21M31S**
test_time_spring_type3_st_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 00:10:20 EST = **P-0Y0M0DT5H31M52S**
test_time_spring_type3_st_type2_st: DIFF: 2010-03-14 00:15:35 EST - 2010-03-14 00:10:20 EST = **P+0Y0M0DT0H5M15S**
test_time_spring_type3_st_type2_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M0DT2H6M35S**
test_time_spring_type3_st_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M1DT18H49M39S**
test_time_spring_type3_dt_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT7H38M27S**
test_time_spring_type3_dt_type2_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT2H6M35S**
test_time_spring_type3_dt_type2_dt: DIFF: 2010-03-14 05:19:56 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M0DT2H3M1S**
test_time_spring_type3_dt_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M1DT16H43M4S**
test_time_spring_type3_post_type2_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M2DT0H21M31S**
test_time_spring_type3_post_type2_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT18H49M39S**
test_time_spring_type3_post_type2_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT16H43M4S**
test_time_spring_type3_post_type2_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-15 18:57:55 EDT = **P+0Y0M0DT1H2M4S**
test_time_spring_type3_stsec_type2_dtsec: DIFF: 2010-03-14 03:00:00 EDT - 2010-03-14 01:59:59 EST = **P+0Y0M0DT0H0M1S**
test_time_spring_type3_dtsec_type2_stsec: DIFF: 2010-03-14 01:59:59 EST - 2010-03-14 03:00:00 EDT = **P-0Y0M0DT0H0M1S**"##,
    );
}

/// ext/date/tests/DateTime_sub-fall-type3-type2.phpt
#[test]
fn datetime_sub_fall_type3_type2() {
    assert_text(
        &examine_diff(FALL_TYPE3_TYPE2, Show::Sub),
        r##"test_time_fall_type3_prev_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P+0Y1M2DT16H19M40S = **2010-10-04 02:18:48 EDT**
test_time_fall_type3_prev_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P+0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type2_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT7H36M16S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT9H38M27S = **2010-11-06 17:38:28 EST**
test_time_fall_type3_prev_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EST**
test_time_fall_type3_dt_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type2_dt: SUB: 2010-11-07 00:15:35 EDT - P+0Y0M0DT0H5M15S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_dt_type2_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT2H4M24S = **2010-11-06 23:10:20 EST**
test_time_fall_type3_dt_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT4H6M35S = **2010-11-06 23:10:20 EST**
test_time_fall_type3_dt_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT20H49M39S = **2010-11-06 23:10:20 EST**
test_time_fall_type3_redodt_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type2_redodt: SUB: 2010-11-07 01:15:35 EDT - P+0Y0M0DT0H3M2S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redodt_type2_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT1H2M11S = **2010-11-07 00:12:33 EST**
test_time_fall_type3_redodt_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT3H4M22S = **2010-11-07 00:12:33 EST**
test_time_fall_type3_redodt_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT19H47M26S = **2010-11-07 00:12:33 EST**
test_time_fall_type3_redost_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT7H36M16S = **2010-11-07 02:14:44 EDT**
test_time_fall_type3_redost_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT2H4M24S = **2010-11-07 02:14:44 EDT**
test_time_fall_type3_redost_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT1H2M11S = **2010-11-07 02:14:44 EDT**
test_time_fall_type3_redost_type2_redost: SUB: 2010-11-07 01:16:54 EST - P+0Y0M0DT0H2M10S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redost_type2_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redost_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_st_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT9H38M27S = **2010-11-07 04:16:55 EDT**
test_time_fall_type3_st_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT4H6M35S = **2010-11-07 04:16:55 EDT**
test_time_fall_type3_st_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT3H4M22S = **2010-11-07 04:16:55 EDT**
test_time_fall_type3_st_type2_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_st_type2_st: SUB: 2010-11-07 05:19:56 EST - P+0Y0M0DT2H3M1S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_st_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_post_type2_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EDT**
test_time_fall_type3_post_type2_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EDT**
test_time_fall_type3_post_type2_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EDT**
test_time_fall_type3_post_type2_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_post_type2_st: SUB: 2010-11-07 03:16:55 EST - P-0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_post_type2_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M0DT1H2M4S = **2010-11-08 18:57:55 EST**
test_time_fall_type3_dtsec_type2_stsec: SUB: 2010-11-07 01:00:00 EST - P+0Y0M0DT0H0M1S = **2010-11-07 00:59:59 EST**
test_time_fall_type3_stsec_type2_dtsec: SUB: 2010-11-07 01:59:59 EDT - P-0Y0M0DT0H0M1S = **2010-11-07 02:00:00 EDT**"##,
    );
}

/// ext/date/tests/DateTime_sub-spring-type3-type2.phpt
#[test]
fn datetime_sub_spring_type3_type2() {
    assert_text(
        &examine_diff(SPRING_TYPE3_TYPE2, Show::Sub),
        r##"test_time_spring_type3_prev_type2_prev: SUB: 2010-03-13 18:38:28 EST - P+0Y1M2DT16H19M40S = **2010-02-11 02:18:48 EST**
test_time_spring_type3_prev_type2_st: SUB: 2010-03-14 00:10:20 EST - P+0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_prev_type2_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT7H38M27S = **2010-03-13 19:38:28 EDT**
test_time_spring_type3_prev_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EDT**
test_time_spring_type3_st_type2_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_st_type2_st: SUB: 2010-03-14 00:15:35 EST - P+0Y0M0DT0H5M15S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_st_type2_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT2H6M35S = **2010-03-14 01:10:20 EDT**
test_time_spring_type3_st_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT18H49M39S = **2010-03-14 01:10:20 EDT**
test_time_spring_type3_dt_type2_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT7H38M27S = **2010-03-14 02:16:55 EST**
test_time_spring_type3_dt_type2_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M0DT2H6M35S = **2010-03-14 02:16:55 EST**
test_time_spring_type3_dt_type2_dt: SUB: 2010-03-14 05:19:56 EDT - P+0Y0M0DT2H3M1S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_dt_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_post_type2_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EST**
test_time_spring_type3_post_type2_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EST**
test_time_spring_type3_post_type2_dt: SUB: 2010-03-14 03:16:55 EDT - P-0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_post_type2_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M0DT1H2M4S = **2010-03-15 18:57:55 EDT**
test_time_spring_type3_stsec_type2_dtsec: SUB: 2010-03-14 03:00:00 EDT - P+0Y0M0DT0H0M1S = **2010-03-14 02:59:59 EDT**
test_time_spring_type3_dtsec_type2_stsec: SUB: 2010-03-14 01:59:59 EST - P-0Y0M0DT0H0M1S = **2010-03-14 02:00:00 EST**"##,
    );
}

const FORMAT_BASIC: [&str; 9] = [
    "F j, Y, g:i a",
    "m.d.y",
    "j, n, Y",
    "Ymd",
    "h-i-s, j-m-y, it is w Day",
    "\\i\\t \\i\\s \\t\\h\\e jS \\d\\a\\y.",
    "D M j G:i:s T Y",
    "H:m:s \\m \\i\\s\\ \\m\\o\\n\\t\\h",
    "H:i:s",
];

/// ext/date/tests/DateTime_format_basic1.phpt
#[test]
fn datetime_format_basic1() {
    let date = DT::new("2005-07-14 22:30:41", "Europe/London");
    let mut out = String::from("*** Testing DateTime::format() : basic functionality ***\n");
    for f in FORMAT_BASIC {
        out.push_str(&vd_str(&date.format(f)));
    }
    assert_text(
        &out,
        r##"*** Testing DateTime::format() : basic functionality ***
string(23) "July 14, 2005, 10:30 pm"
string(8) "07.14.05"
string(11) "14, 7, 2005"
string(8) "20050714"
string(39) "10-30-41, 14-07-05, 3031 3041 4 Thupm05"
string(19) "it is the 14th day."
string(28) "Thu Jul 14 22:30:41 BST 2005"
string(19) "22:07:41 m is month"
string(8) "22:30:41""##,
    );
}

/// ext/date/tests/date_format_basic1.phpt
#[test]
fn date_format_basic1() {
    let date = DT::new("2005-07-14 22:30:41", "Europe/London");
    let mut out = String::from("*** Testing date_format() : basic functionality ***\n");
    for f in FORMAT_BASIC {
        out.push_str(&vd_str(&date.format(f)));
    }
    assert_text(
        &out,
        r##"*** Testing date_format() : basic functionality ***
string(23) "July 14, 2005, 10:30 pm"
string(8) "07.14.05"
string(11) "14, 7, 2005"
string(8) "20050714"
string(39) "10-30-41, 14-07-05, 3031 3041 4 Thupm05"
string(19) "it is the 14th day."
string(28) "Thu Jul 14 22:30:41 BST 2005"
string(19) "22:07:41 m is month"
string(8) "22:30:41""##,
    );
}

/// ext/date/tests/DateTime_modify_invalid_format.phpt
#[test]
fn datetime_modify_invalid_format() {
    let mut datetime = DT::new("now", "UTC");
    let before = datetime.t.sse;
    assert!(!datetime.modify(""));
    assert_eq!(datetime.t.sse, before);
    // "Failed to parse time string () at position 0 ( ): Empty string"
    let e = php::date_modify(&mut datetime.t, b"").unwrap_err();
    assert_eq!(e.errors[0].position, 0);
    assert_eq!(e.errors[0].message.to_string(), "Empty string");
}

/// ext/date/tests/DateTime_setTimezone_basic1.phpt
#[test]
fn datetime_settimezone_basic1() {
    let mut datetime = DT::new("2009-01-30 17:57:32", "Europe/London");
    let mut out = String::from("*** Testing DateTime::setTimezone() : basic functionality ***\n");
    let _ = writeln!(out, "Default timezone: {}", datetime.timezone_name());
    datetime.set_timezone("America/Los_Angeles");
    let _ = writeln!(out, "New timezone: {}", datetime.timezone_name());
    assert_text(
        &out,
        r##"*** Testing DateTime::setTimezone() : basic functionality ***
Default timezone: Europe/London
New timezone: America/Los_Angeles"##,
    );
}

/// ext/date/tests/bug-gh9106.phpt
#[test]
fn bug_gh9106() {
    let start = DT::new("2020-01-01 00:00:00 UTC", "UTC");
    let mut one_and_half_sec = php::date_interval_create("PT1S").unwrap();
    set_f(&mut one_and_half_sec, 0.5);
    let mut out = vd_int(start.timestamp());
    let mut t = start.clone();
    for _ in 0..4 {
        t.add_interval(&one_and_half_sec);
        out.push_str(&vd_int(t.timestamp()));
    }
    assert_text(
        &out,
        r##"int(1577836800)
int(1577836801)
int(1577836803)
int(1577836804)
int(1577836806)"##,
    );
}

/// ext/date/tests/bug20382-2.phpt
#[test]
fn bug20382_2() {
    let tests: [(&str, i64, i64, i64, i64, i64, i64); 25] = [
        ("Europe/Andorra", 17, 17, 17, 1, 24764, 1970),
        ("Asia/Dubai", 17, 17, 17, 1, 1, 1970),
        ("Asia/Kabul", 17, 17, 17, 1, 1, 1970),
        ("America/Antigua", 17, 17, 17, 1, 1, 1970),
        ("America/Anguilla", 17, 17, 17, 1, 1, 1970),
        ("Europe/Tirane", 17, 17, 17, 1, 4849, 1970),
        ("Asia/Yerevan", 17, 17, 17, 1, 24764, 1970),
        ("America/Curacao", 17, 17, 17, 1, 1, 1970),
        ("Africa/Luanda", 17, 17, 17, 1, 1, 1970),
        ("Antarctica/McMurdo", 17, 17, 17, 1, 24743, 1970),
        ("Australia/Adelaide", 17, 17, 17, 1, 1, 1971),
        ("Australia/Darwin", 17, 17, 17, 1, 88, 1971),
        ("Australia/Perth", 17, 17, 17, 1, 1, 1971),
        ("America/Aruba", 17, 17, 17, 1, 88, 1971),
        ("Asia/Baku", 17, 17, 17, 1, 1, 1971),
        ("Europe/Sarajevo", 17, 17, 17, 1, 1, 1971),
        ("America/Barbados", 17, 17, 17, 1, 1, 1971),
        ("Asia/Dacca", 17, 17, 17, 1, 1, 1971),
        ("Europe/Brussels", 17, 17, 17, 1, 1, 1971),
        ("Africa/Ouagadougou", 17, 17, 17, 1, 88, 1971),
        ("Europe/Tirane", 17, 17, 17, 1, 4849, 1970),
        ("America/Buenos_Aires", 17, 17, 17, 1, 1734, 1970),
        ("America/Rosario", 17, 17, 17, 1, 1734, 1970),
        ("Europe/Vienna", 17, 17, 17, 1, 3743, 1970),
        ("Asia/Baku", 17, 17, 17, 1, 9490, 1970),
    ];
    let mut out = String::new();
    for (zone, h, i, s, m, d, y) in tests {
        let _ = writeln!(out, "{zone}");
        let timestamp = mktime(h, i, s, m, d, y, zone);
        let _ = writeln!(out, "ts     = {}", date("l Y-m-d H:i:s T", timestamp, zone));
        let r = strtotime("first monday", timestamp, zone).unwrap();
        let _ = writeln!(out, "result = {}", date("l Y-m-d H:i:s T", r, zone));
        out.push_str("wanted = Monday            00:00:00\n\n");
    }
    assert_textf(
        &out,
        r##"Europe/Andorra
ts     = Monday 2037-10-19 17:17:17 CEST
result = Monday 2037-10-26 00:00:00 CET
wanted = Monday            00:00:00

Asia/Dubai
ts     = Thursday 1970-01-01 17:17:17 %s
result = Monday 1970-01-05 00:00:00 %s
wanted = Monday            00:00:00

Asia/Kabul
ts     = Thursday 1970-01-01 17:17:17 %s
result = Monday 1970-01-05 00:00:00 %s
wanted = Monday            00:00:00

America/Antigua
ts     = Thursday 1970-01-01 17:17:17 AST
result = Monday 1970-01-05 00:00:00 AST
wanted = Monday            00:00:00

America/Anguilla
ts     = Thursday 1970-01-01 17:17:17 AST
result = Monday 1970-01-05 00:00:00 AST
wanted = Monday            00:00:00

Europe/Tirane
ts     = Monday 1983-04-11 17:17:17 CET
result = Monday 1983-04-18 01:00:00 CEST
wanted = Monday            00:00:00

Asia/Yerevan
ts     = Monday 2037-10-19 17:17:17 +04
result = Monday 2037-10-26 00:00:00 +04
wanted = Monday            00:00:00

America/Curacao
ts     = Thursday 1970-01-01 17:17:17 AST
result = Monday 1970-01-05 00:00:00 AST
wanted = Monday            00:00:00

Africa/Luanda
ts     = Thursday 1970-01-01 17:17:17 WAT
result = Monday 1970-01-05 00:00:00 WAT
wanted = Monday            00:00:00

Antarctica/McMurdo
ts     = Monday 2037-09-28 17:17:17 NZDT
result = Monday 2037-10-05 00:00:00 NZDT
wanted = Monday            00:00:00

Australia/Adelaide
ts     = Friday 1971-01-01 17:17:17 ACST
result = Monday 1971-01-04 00:00:00 ACST
wanted = Monday            00:00:00

Australia/Darwin
ts     = Monday 1971-03-29 17:17:17 ACST
result = Monday 1971-04-05 00:00:00 ACST
wanted = Monday            00:00:00

Australia/Perth
ts     = Friday 1971-01-01 17:17:17 AWST
result = Monday 1971-01-04 00:00:00 AWST
wanted = Monday            00:00:00

America/Aruba
ts     = Monday 1971-03-29 17:17:17 AST
result = Monday 1971-04-05 00:00:00 AST
wanted = Monday            00:00:00

Asia/Baku
ts     = Friday 1971-01-01 17:17:17 +04
result = Monday 1971-01-04 00:00:00 +04
wanted = Monday            00:00:00

Europe/Sarajevo
ts     = Friday 1971-01-01 17:17:17 CET
result = Monday 1971-01-04 00:00:00 CET
wanted = Monday            00:00:00

America/Barbados
ts     = Friday 1971-01-01 17:17:17 AST
result = Monday 1971-01-04 00:00:00 AST
wanted = Monday            00:00:00

Asia/Dacca
ts     = Friday 1971-01-01 17:17:17 %s
result = Monday 1971-01-04 00:00:00 %s
wanted = Monday            00:00:00

Europe/Brussels
ts     = Friday 1971-01-01 17:17:17 CET
result = Monday 1971-01-04 00:00:00 CET
wanted = Monday            00:00:00

Africa/Ouagadougou
ts     = Monday 1971-03-29 17:17:17 GMT
result = Monday 1971-04-05 00:00:00 GMT
wanted = Monday            00:00:00

Europe/Tirane
ts     = Monday 1983-04-11 17:17:17 CET
result = Monday 1983-04-18 01:00:00 CEST
wanted = Monday            00:00:00

America/Buenos_Aires
ts     = Monday 1974-09-30 17:17:17 %s
result = Monday 1974-10-07 00:00:00 %s
wanted = Monday            00:00:00

America/Rosario
ts     = Monday 1974-09-30 17:17:17 %s
result = Monday 1974-10-07 00:00:00 %s
wanted = Monday            00:00:00

Europe/Vienna
ts     = Monday 1980-03-31 17:17:17 CET
result = Monday 1980-04-07 00:00:00 CEST
wanted = Monday            00:00:00

Asia/Baku
ts     = Monday 1995-12-25 17:17:17 +04
result = Monday 1996-01-01 00:00:00 +04
wanted = Monday            00:00:00"##,
    );
}

/// ext/date/tests/bug26320.phpt
#[test]
fn bug26320() {
    let zone = "GMT0";
    let mut out = String::new();
    for s in ["2003-11-19T12:30:42", "2003-11-19T12:30:42Z"] {
        out.push_str(&date(
            "Y-m-d H:i:s\n",
            strtotime_abs(s, zone).unwrap(),
            zone,
        ));
    }
    assert_text(
        &out,
        r##"2003-11-19 12:30:42
2003-11-19 12:30:42"##,
    );
}

/// ext/date/tests/bug29150.phpt
#[test]
fn bug29150() {
    let ts = strtotime_abs("20 VI. 2005", "GMT").unwrap();
    assert_eq!(gmdate("Y-m-d H:i:s", ts), "2005-06-20 00:00:00");
}

/// ext/date/tests/bug32270.phpt
#[test]
fn bug32270() {
    let zone = "America/Los_Angeles";
    let mut out = String::new();
    for (ts, s) in [
        (-2_145_888_000, "Jan 1 1902"),
        (-631_123_200, "Jan 1 1950"),
        (946_713_600, "Jan 1 2000"),
    ] {
        let _ = writeln!(out, "{}", date("m/d/Y H:i:s T", ts, zone));
        let _ = writeln!(out, "{}", strtotime_abs(s, zone).unwrap());
    }
    assert_text(
        &out,
        r##"01/01/1902 00:00:00 PST
-2145888000
01/01/1950 00:00:00 PST
-631123200
01/01/2000 00:00:00 PST
946713600"##,
    );
}

/// ext/date/tests/bug33415-1.phpt
#[test]
fn bug33415_1() {
    let mut out = String::from(
        "TZ=America/Jujuy  - Is it OK for this to be 2 AM, rather than 1\nAM as per most DST transitions?\n",
    );
    let zone = "America/Jujuy";
    let t = mktime(17, 17, 17, 1, 7593, 1970, zone);
    let _ = writeln!(out, "tStamp={}", date("l Y-m-d H:i:s T I", t, zone));
    let r = strtotime("next Monday", t, zone).unwrap();
    let _ = writeln!(out, "result={}", date("l Y-m-d H:i:s T I", r, zone));
    out.push_str("wanted=Monday            00:00:00\n\n");

    out.push_str("TZ=Asia/Tbilisi - Is it OK for this to be 2 AM?\n");
    let zone = "Asia/Tbilisi";
    let t = mktime(17, 17, 17, 1, 12863, 1970, zone);
    let _ = writeln!(out, "tStamp={}", date("l Y-m-d H:i:s T I", t, zone));
    let r = strtotime("next Sunday", t, zone).unwrap();
    let _ = writeln!(out, "result={}", date("l Y-m-d H:i:s T I", r, zone));
    out.push_str("wanted=Sunday            00:00:00\n\n");
    assert_text(
        &out,
        r##"TZ=America/Jujuy  - Is it OK for this to be 2 AM, rather than 1
AM as per most DST transitions?
tStamp=Monday 1990-10-15 17:17:17 -04 0
result=Monday 1990-10-22 00:00:00 -04 0
wanted=Monday            00:00:00

TZ=Asia/Tbilisi - Is it OK for this to be 2 AM?
tStamp=Sunday 2005-03-20 17:17:17 +03 0
result=Sunday 2005-03-27 00:00:00 +03 0
wanted=Sunday            00:00:00"##,
    );
}

/// ext/date/tests/bug33563.phpt
#[test]
fn bug33563() {
    let zone = "GMT";
    let curr = date(
        "Y-m-d H:i:s",
        strtotime_abs("2005-06-30 21:04:23", zone).unwrap(),
        zone,
    );
    let base = strtotime_abs(&curr, zone).unwrap();
    let after = date(
        "Y-m-d H:i:s",
        strtotime("+1 month", base, zone).unwrap(),
        zone,
    );
    assert_eq!(
        format!("strCurrDate:{curr} strMonAfter:{after}"),
        "strCurrDate:2005-06-30 21:04:23 strMonAfter:2005-07-30 21:04:23"
    );
}

/// ext/date/tests/bug34676.phpt
#[test]
fn bug34676() {
    let mut out = String::new();
    for test in ["noon", "midnight"] {
        let t = strtotime_abs(&format!("2005-12-22 {test}"), "UTC").unwrap();
        let _ = writeln!(out, "{test:<10} => {}", date(DATE_ISO8601, t, "UTC"));
    }
    assert_text(
        &out,
        r##"noon       => 2005-12-22T12:00:00+0000
midnight   => 2005-12-22T00:00:00+0000"##,
    );
}

/// ext/date/tests/bug35425.phpt
#[test]
fn bug35425() {
    let zone = "America/Montreal";
    let time = mktime(1, 1, 1, 1, 1, 2005, zone);
    let mut out = String::new();
    for v in [
        'B', 'd', 'h', 'H', 'i', 'I', 'L', 'm', 's', 't', 'U', 'w', 'W', 'y', 'Y', 'z', 'Z',
    ] {
        out.push_str(&vd_int(i64::from(idate(v, time, zone).unwrap())));
    }
    assert_text(
        &out,
        r##"int(292)
int(1)
int(1)
int(1)
int(1)
int(0)
int(0)
int(1)
int(1)
int(31)
int(1104559261)
int(6)
int(53)
int(5)
int(2005)
int(0)
int(-18000)"##,
    );
}

/// ext/date/tests/bug35705.phpt
#[test]
fn bug35705() {
    let t = strtotime_abs("2000-10-10T10:12:30.000", "UTC").unwrap();
    assert_eq!(date(DATE_ISO8601, t, "UTC"), "2000-10-10T10:12:30+0000");
}

/// ext/date/tests/bug38229.phpt
#[test]
fn bug38229() {
    let f = |s: &str| date("Y-m", strtotime_abs(s, "GMT").unwrap(), "GMT");
    assert_eq!(f("2006-1"), "2006-01");
    assert_eq!(f("2006-03"), "2006-03");
    assert_eq!(f("2006-12"), "2006-12");
}

/// ext/date/tests/bug41599.phpt
#[test]
fn bug41599() {
    let mut start = DT::new("2008-01-17 last Monday", "Europe/London");
    let mut out = String::new();
    let _ = writeln!(out, "{}", start.format("Y-m-d H:i:s"));
    assert!(start.modify("Tuesday"));
    let _ = writeln!(out, "{}", start.format("Y-m-d H:i:s"));
    start.set_time(4, 0, 0, 0);
    let _ = writeln!(out, "{}", start.format("Y-m-d H:i:s"));
    start.set_time(8, 0, 0, 0);
    let _ = writeln!(out, "{}", start.format("Y-m-d H:i:s"));
    assert_text(
        &out,
        r##"2008-01-14 00:00:00
2008-01-15 00:00:00
2008-01-15 04:00:00
2008-01-15 08:00:00"##,
    );
}

/// ext/date/tests/bug43003.phpt
#[test]
fn bug43003() {
    let zone = "Europe/Oslo";
    let mut out = String::new();
    let mut d = DT::new_in("@0", zone, zone);
    let _ = writeln!(out, "{}: {}", d.timezone_name(), d.format("Y-m-d H:i:s"));
    d.set_timezone("UTC");
    let _ = writeln!(out, "{}: {}", d.timezone_name(), d.format("Y-m-d H:i:s"));
    d.set_timezone(zone);
    let _ = writeln!(out, "{}: {}", d.timezone_name(), d.format("Y-m-d H:i:s"));
    let mut d = DT::new("@0", zone);
    let _ = writeln!(out, "{}: {}", d.timezone_name(), d.format("Y-m-d H:i:s"));
    d.set_timezone(zone);
    let _ = writeln!(out, "{}: {}", d.timezone_name(), d.format("Y-m-d H:i:s"));
    assert_text(
        &out,
        r##"+00:00: 1970-01-01 00:00:00
UTC: 1970-01-01 00:00:00
Europe/Oslo: 1970-01-01 01:00:00
+00:00: 1970-01-01 00:00:00
Europe/Oslo: 1970-01-01 01:00:00"##,
    );
}

/// ext/date/tests/bug44562.phpt (partial: the deprecation notice and exception classes
/// are not expressible)
#[test]
fn bug44562() {
    // DatePeriod::createFromISO8601String('2D') => "Unknown or bad format (2D)"
    assert!(!php_timelib::strtointerval("2D").errors.errors.is_empty());

    let begin = DT::new("2008-07-20T22:44:53+0200", "Europe/Oslo");
    let interval = php::date_interval_create_from_date_string("1 day").unwrap();
    let recurrences = 10 + 1; // + include_start_date
    let mut current = begin.t.clone();
    let mut out = String::new();
    for i in 0..recurrences {
        if i > 0 {
            period_advance(&mut current, &interval.diff);
        }
        out.push_str(&vd_str(&fmt_time(&current, DATE_ISO8601)));
    }
    assert_text(
        &out,
        r#"string(24) "2008-07-20T22:44:53+0200"
string(24) "2008-07-21T22:44:53+0200"
string(24) "2008-07-22T22:44:53+0200"
string(24) "2008-07-23T22:44:53+0200"
string(24) "2008-07-24T22:44:53+0200"
string(24) "2008-07-25T22:44:53+0200"
string(24) "2008-07-26T22:44:53+0200"
string(24) "2008-07-27T22:44:53+0200"
string(24) "2008-07-28T22:44:53+0200"
string(24) "2008-07-29T22:44:53+0200"
string(24) "2008-07-30T22:44:53+0200""#,
    );
}

/// ext/date/tests/bug45554.phpt
#[test]
fn bug45554() {
    let format = "m-d-Y H:i:s.u T";
    let mut out = String::new();
    for s in [
        "03-15-2005 12:22:29.000000 PST",
        "03-15-2005 12:22:29.001001 PST",
        "03-15-2005 12:22:29.0010 PST",
    ] {
        let d = DT::from_format(format, s, None, "UTC").unwrap();
        let _ = writeln!(out, "{}", d.format(format));
    }
    assert_text(
        &out,
        r##"03-15-2005 12:22:29.000000 PST
03-15-2005 12:22:29.001001 PST
03-15-2005 12:22:29.001000 PST"##,
    );
}

/// ext/date/tests/bug46874.phpt
#[test]
fn bug46874() {
    let iso = php_timelib::strtointerval("R5/2008-03-01T13:00:00Z/P1Y2M10DT2H30M");
    assert!(iso.errors.errors.is_empty());
    let mut start = iso.begin.unwrap();
    start.update_ts(None);
    let interval = iso.period.unwrap();
    let recurrences = i64::from(iso.recurrences.unwrap()) + 1; // + include_start_date
    let mut out = String::new();
    for pass in 0..2 {
        if pass > 0 {
            out.push('\n');
        }
        let mut current = start.clone();
        for i in 0..recurrences {
            if i > 0 {
                period_advance(&mut current, &interval);
            }
            out.push_str(&fmt_time(&current, "Y-m-d H:i:s\n"));
        }
    }
    assert_text(
        &out,
        r##"2008-03-01 13:00:00
2009-05-11 15:30:00
2010-07-21 18:00:00
2011-10-01 20:30:00
2012-12-11 23:00:00
2014-02-22 01:30:00

2008-03-01 13:00:00
2009-05-11 15:30:00
2010-07-21 18:00:00
2011-10-01 20:30:00
2012-12-11 23:00:00
2014-02-22 01:30:00"##,
    );
}

/// ext/date/tests/bug49059.phpt
#[test]
fn bug49059() {
    let zone = "Asia/Calcutta";
    let mut date1 = DT::new("2009-03-27", zone);
    let date2 = DT::new("2009-03-01", zone);
    let mut out = String::new();
    let _ = writeln!(out, "$date1 at init: {}", date1.format("Y-m-d"));
    let _ = writeln!(out, "$date2 at init: {}", date2.format("Y-m-d"));
    let diff = date1.diff(&date2);
    let _ = writeln!(out, "$date1 after first diff: {}", date1.format("Y-m-d"));
    let _ = writeln!(out, "$diff->days after first diff: {}", diff.diff.days);
    date1.sub("P2D");
    let _ = writeln!(out, "$date1 after sub: {}", date1.format("Y-m-d"));
    let diff = date1.diff(&date2);
    let _ = writeln!(
        out,
        "$date1 after second diff (called at $date1): {}",
        date1.format("Y-m-d")
    );
    let _ = writeln!(out, "$diff->days after second diff: {}", diff.diff.days);
    let diff = date2.diff(&date1);
    let _ = writeln!(
        out,
        "$date1 after third diff (called at $date2): {}",
        date1.format("Y-m-d")
    );
    let _ = writeln!(out, "$diff->days after third diff: {}", diff.diff.days);
    assert_text(
        &out,
        r##"$date1 at init: 2009-03-27
$date2 at init: 2009-03-01
$date1 after first diff: 2009-03-27
$diff->days after first diff: 26
$date1 after sub: 2009-03-25
$date1 after second diff (called at $date1): 2009-03-25
$diff->days after second diff: 24
$date1 after third diff (called at $date2): 2009-03-25
$diff->days after third diff: 24"##,
    );
}

/// ext/date/tests/bug50055-002.phpt (the exception is `date_sub()` returning `None`)
#[test]
fn bug50055_002() {
    let now = "2010-03-07 13:21:38 UTC";
    let mut out = String::new();
    let i = php::date_interval_create_from_date_string("third Tuesday of next month").unwrap();
    let i2 = php::date_interval_create_from_date_string("third Tuesday of last month").unwrap();
    for iv in [&i, &i2] {
        let mut da = DT::new(now, "UTC");
        let ds = DT::new(now, "UTC");
        let _ = writeln!(out, "{}", da.format(DATE_ISO8601));
        da.add_interval(iv);
        let _ = writeln!(out, "{}", da.format(DATE_ISO8601));
        // Both subtractions use `$i`.
        if php::date_sub(&ds.t, &i).is_none() {
            out.push_str("DateInvalidOperationException: DateTime::sub(): Only non-special relative time specifications are supported for subtraction\n");
        }
    }
    assert_textf(
        &out,
        r##"2010-03-07T13:21:38+0000
2010-04-20T13:21:38+0000
DateInvalidOperationException: DateTime::sub(): Only non-special relative time specifications are supported for subtraction
2010-03-07T13:21:38+0000
2010-02-16T13:21:38+0000
DateInvalidOperationException: DateTime::sub(): Only non-special relative time specifications are supported for subtraction"##,
    );
}

/// ext/date/tests/bug51819.phpt
#[test]
fn bug51819() {
    let mut seen: Vec<&str> = Vec::new();
    for e in php_timelib::timezone_abbreviations_list() {
        // PHP's `timezone_id` is NULL for the military zones, which concatenates as "".
        let id = e.full_tz_name.unwrap_or("");
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    assert!(seen.len() > 100);
    let mut failures = Vec::new();
    for tz in &seen {
        let s = format!("2010-05-15 00:00:00 {tz}");
        if let Err(e) = DT::try_new(&s, None, "UTC") {
            failures.push((s, e));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// ext/date/tests/bug51819.phpt: `timezone_abbreviations_list()` gives `'timezone_id' =>
/// NULL` for the military zones (`{ "a", 0, 3600, NULL }` in `timezonemap.h`), so the
/// script parses `"2010-05-15 00:00:00 "`; and `timezone_name_from_abbr("a")` is `false`.
#[test]
fn bug51819_null_timezone_id() {
    let nulls: Vec<&str> = php_timelib::timezone_abbreviations_list()
        .iter()
        .filter(|e| e.full_tz_name.is_none())
        .map(|e| e.name)
        .collect();
    // the 25 military zones "a".."z" (without "j")
    assert_eq!(nulls.len(), 25, "{nulls:?}");
    assert!(nulls.iter().all(|n| n.len() == 1));
    assert_eq!(php_timelib::timezone_id_from_abbr("a", -1, 0), None);
    for e in php_timelib::timezone_abbreviations_list() {
        let s = format!("2010-05-15 00:00:00 {}", e.full_tz_name.unwrap_or(""));
        assert!(DT::try_new(&s, None, "UTC").is_ok(), "{s}");
    }
}

/// ext/date/tests/bug52454.phpt
#[test]
fn bug52454() {
    let mut end_of_week = DT::new("2010-07-27 09:46:49", "Europe/London");
    assert!(end_of_week.modify("this week +6 days"));
    let mut out = String::new();
    let _ = writeln!(out, "{}", end_of_week.format("Y-m-d H:i:s"));
    let _ = writeln!(out, "{}", end_of_week.format("U"));
    let _ = writeln!(out, "{}", end_of_week.timestamp());
    let _ = writeln!(out, "{}", end_of_week.format("Y-m-d H:i:s"));
    assert_text(
        &out,
        r##"2010-08-01 09:46:49
1280652409
1280652409
2010-08-01 09:46:49"##,
    );
}

/// ext/date/tests/bug54597.phpt
#[test]
fn bug54597() {
    for s in ["January 0099", "January 1, 0099", "0099-01"] {
        let d = DT::new_in(s, "Europe/Amsterdam", "Europe/London");
        assert_eq!(d.format("Y"), "0099", "{s}");
    }
}

/// ext/date/tests/bug62561.phpt
#[test]
fn bug62561() {
    let ts = DT::new_in("@1341115200", "America/New_York", "UTC");
    let mut day_from_ts = DT::new_in("@1341115200", "America/New_York", "UTC");
    day_from_ts.add("P1D");
    assert_eq!(ts.format("Y-m-d H:i:s"), "2012-07-01 04:00:00");
    assert_eq!(day_from_ts.format("Y-m-d H:i:s"), "2012-07-02 04:00:00");
}

/// ext/date/tests/bug63435.phpt
#[test]
fn bug63435() {
    let mut bad = Vec::new();
    for i in 1..999 {
        let datetime = DT::from_format("u", &format!("{i:06}"), None, "UTC").unwrap();
        let res = datetime.format("u");
        if res.parse::<i64>().ok() != Some(i) {
            bad.push(format!("{i} != {res}"));
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

/// ext/date/tests/bug65003.phpt
#[test]
fn bug65003() {
    let zone = "Europe/Moscow";
    let datetime1 = DT::new("13-03-01", zone);
    let datetime2 = DT::new("13-04-01", zone);
    let datetime3 = DT::new("13-03-02", zone);
    let datetime4 = DT::new("13-04-02", zone);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}",
        interval_format(&datetime2.diff(&datetime1), "%m month, %d days")
    );
    let _ = writeln!(
        out,
        "{}",
        interval_format(&datetime4.diff(&datetime3), "%m month, %d days")
    );
    assert_text(
        &out,
        r##"1 month, 0 days
1 month, 0 days"##,
    );
}

/// ext/date/tests/bug66019.phpt
#[test]
fn bug66019() {
    let d = DT::new_in("2012-02-02T10", "Europe/Amsterdam", "UTC");
    assert_eq!(d.format("j F Y H:i"), "2 February 2012 10:00");
}

/// ext/date/tests/bug68078_negative.phpt
#[test]
fn bug68078_negative() {
    let mk = |s: &str| {
        let mut d = DT::from_format("U.u", s, None, "UTC").unwrap();
        assert!(d.modify("-5 seconds"));
        d
    };
    let early1 = mk("1.8642");
    let early2 = mk("1.2768");
    let early3 = mk("1.2768");
    use std::cmp::Ordering::*;
    assert_ne!(early1.t.compare(&early2.t), Equal);
    assert_eq!(early1.t.compare(&early2.t), Greater);
    assert_eq!(early2.t.compare(&early1.t), Less);
    assert_eq!(early2.t.compare(&early3.t), Equal);
}

/// ext/date/tests/bug72963.phpt (partial: the null-byte ValueErrors are not
/// expressible)
#[test]
fn bug72963() {
    let s = "8/8/2016";
    // run-tests.php trims the output, so the leading "\n" is dropped.
    let mut out = format!("Covering string: {s}\n\n");
    let d1 = DT::from_format("!m/d/Y", s, None, "UTC").unwrap();
    let d2 = DT::from_format("!m/d/Y", s, None, "UTC").unwrap();
    out.push_str(&d1.var_dump("DateTime", 1));
    out.push_str(&d2.var_dump("DateTimeImmutable", 2));
    out.push_str(&date_parse_from_format("m/d/Y", s).var_dump());
    let expected = r##"Covering string: 8/8/2016

object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2016-08-08 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTimeImmutable)#2 (3) {
  ["date"]=>
  string(26) "2016-08-08 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
array(12) {
  ["year"]=>
  int(2016)
  ["month"]=>
  int(8)
  ["day"]=>
  int(8)
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

Covering string: 8/8/2016\0asf

ValueError: DateTime::createFromFormat(): Argument #2 ($datetime) must not contain any null bytes
ValueError: DateTimeImmutable::createFromFormat(): Argument #2 ($datetime) must not contain any null bytes
ValueError: date_parse_from_format(): Argument #2 ($datetime) must not contain any null bytes
NULL
NULL
NULL"##;
    let expected = &expected[..expected.find("\nCovering string: 8/8/2016\\0asf").unwrap()];
    assert_text(&out, expected);
}

/// ext/date/tests/bug73460-002.phpt
#[test]
fn bug73460_002() {
    let zone = "America/New_York";
    let mut out = String::new();
    let start = DT::new("2006-04-02T01:00:00", zone);
    let mut end = DT::new("2006-04-02T04:00:00", zone);
    let mut guard = 0;
    while gt(&end, &start) && guard < 20 {
        end.sub("PT1H");
        let _ = writeln!(out, "{}", end.format("Y-m-d H:i T"));
        guard += 1;
    }
    out.push_str("-----\n");
    let mut start = DT::new("2006-10-29T00:30:00", zone);
    let end = DT::new("2006-10-29T03:00:00", zone);
    let mut guard = 0;
    while gt(&end, &start) && guard < 20 {
        start.add("PT30M");
        let _ = writeln!(out, "{}", start.format("Y-m-d H:i T"));
        guard += 1;
    }
    assert_text(
        &out,
        r##"2006-04-02 03:00 EDT
2006-04-02 01:00 EST
-----
2006-10-29 01:00 EDT
2006-10-29 01:30 EDT
2006-10-29 01:00 EST
2006-10-29 01:30 EST
2006-10-29 02:00 EST
2006-10-29 02:30 EST
2006-10-29 03:00 EST"##,
    );
}

/// ext/date/tests/bug74173.phpt
#[test]
fn bug74173() {
    let utc = DT::new("2016-10-30T00:00:00+00:0", "UTC");
    let mut prg = utc.clone();
    prg.set_timezone("Europe/Prague");
    assert_eq!(prg.format("c"), "2016-10-30T02:00:00+02:00");
    let _ = prg.timestamp();
    assert_eq!(prg.format("c"), "2016-10-30T02:00:00+02:00");
}

/// ext/date/tests/bug75577.phpt
#[test]
fn bug75577() {
    let zone = "Europe/London";
    let d = DT::new("2018-10-09 09:56:45.412000", zone);
    let s = d.format(DATE_RFC3339_EXTENDED);
    let mut out = vd_str(&s);
    let d2 = DT::from_format(DATE_RFC3339_EXTENDED, &s, None, zone).unwrap();
    out.push_str(&d2.var_dump("DateTime", 2));
    assert_textf(
        &out,
        r##"string(29) "2018-10-09T09:56:45.412+01:00"
object(DateTime)#%d (3) {
  ["date"]=>
  string(26) "2018-10-09 09:56:45.412000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:00"
}"##,
    );
}

/// ext/date/tests/bug79396-forward-transition-settime.phpt
#[test]
fn bug79396_forward_transition_settime() {
    let zone = "America/Chicago";
    let mut out = String::new();
    for (h, i, s) in [(2, 0, 0), (2, 30, 0), (3, 0, 0), (1, 59, 59)] {
        let mut date = DT::new("2020-03-08 01:30:00", zone);
        date.set_time(h, i, s, 0);
        let _ = writeln!(out, "{}", date.format("Y-m-d H:i:s T/e - U"));
    }
    assert_text(
        &out,
        r##"2020-03-08 03:00:00 CDT/America/Chicago - 1583654400
2020-03-08 03:30:00 CDT/America/Chicago - 1583656200
2020-03-08 03:00:00 CDT/America/Chicago - 1583654400
2020-03-08 01:59:59 CST/America/Chicago - 1583654399"##,
    );
}

/// ext/date/tests/bug80483.phpt (partial: serialize/unserialize are not expressible)
#[test]
fn bug80483() {
    let mut the_date = DT::new("now", "UTC");
    the_date.set_time(0, 0, 0, 0);
    the_date.set_date(20201, 1, 1);
    assert_textf(
        &the_date.var_dump("DateTime", 1),
        r#"object(DateTime)#%d (%d) {
  ["date"]=>
  string(28) "+20201-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"#,
    );
}

/// ext/date/tests/bug80998.phpt
#[test]
fn bug80998() {
    let mut date = DT::new("2021-04-05 14:00:00", "UTC");
    let mut interval = php::date_interval_create("P0DT10799S").unwrap();
    set_f(&mut interval, 0.999999);
    interval.diff.invert = true;
    date.add_interval(&interval);
    assert_eq!(date.format("Y-m-d H:i:s.u"), "2021-04-05 11:00:00.000001");
}

/// ext/date/tests/date.phpt
#[test]
fn date_phpt() {
    let mut out = String::new();
    for zone in ["UTC", "MET"] {
        for f in ["c", "r"] {
            let _ = writeln!(out, "{f}: {}", date(f, 1_043_324_459, zone));
        }
    }
    assert_text(
        &out,
        r##"c: 2003-01-23T12:20:59+00:00
r: Thu, 23 Jan 2003 12:20:59 +0000
c: 2003-01-23T13:20:59+01:00
r: Thu, 23 Jan 2003 13:20:59 +0100"##,
    );
}

/// ext/date/tests/date_create-relative.phpt
#[test]
fn date_create_relative() {
    let base_time = "28 Feb 2008 12:00:00";
    let offsets = [
        "1460000 days",
        "1460969 days",
        "1460970 days",
        "1460971 days",
        "1462970 days",
        "398 years",
        "399 years",
        "400 years",
        "401 years",
        "39755 years",
        "39999 years",
        "40000 years",
        "40001 years",
        "41010 years",
        "10000000000 seconds",
        "10000000000 minutes",
        "10000000000 hours",
        "10000000000 days",
        "10000000000 months",
        "10000000000 years",
    ];
    let mut out = String::new();
    for offset in offsets {
        for direction in ["+", "-"] {
            let d = dt(&format!("{base_time} {direction}{offset}"), "UTC");
            let s = d.map_or("<false>".to_string(), |d| d.format(DATE_ISO8601));
            let _ = writeln!(out, "{direction}{offset}: {s}");
        }
    }
    assert_text(
        &out,
        r##"+1460000 days: 6005-07-03T12:00:00+0000
-1460000 days: -1990-10-25T12:00:00+0000
+1460969 days: 6008-02-27T12:00:00+0000
-1460969 days: -1992-02-29T12:00:00+0000
+1460970 days: 6008-02-28T12:00:00+0000
-1460970 days: -1992-02-28T12:00:00+0000
+1460971 days: 6008-02-29T12:00:00+0000
-1460971 days: -1992-02-27T12:00:00+0000
+1462970 days: 6013-08-20T12:00:00+0000
-1462970 days: -1998-09-07T12:00:00+0000
+398 years: 2406-02-28T12:00:00+0000
-398 years: 1610-02-28T12:00:00+0000
+399 years: 2407-02-28T12:00:00+0000
-399 years: 1609-02-28T12:00:00+0000
+400 years: 2408-02-28T12:00:00+0000
-400 years: 1608-02-28T12:00:00+0000
+401 years: 2409-02-28T12:00:00+0000
-401 years: 1607-02-28T12:00:00+0000
+39755 years: 41763-02-28T12:00:00+0000
-39755 years: -37747-02-28T12:00:00+0000
+39999 years: 42007-02-28T12:00:00+0000
-39999 years: -37991-02-28T12:00:00+0000
+40000 years: 42008-02-28T12:00:00+0000
-40000 years: -37992-02-28T12:00:00+0000
+40001 years: 42009-02-28T12:00:00+0000
-40001 years: -37993-02-28T12:00:00+0000
+41010 years: 43018-02-28T12:00:00+0000
-41010 years: -39002-02-28T12:00:00+0000
+10000000000 seconds: 2325-01-18T05:46:40+0000
-10000000000 seconds: 1691-04-09T18:13:20+0000
+10000000000 minutes: 21021-05-27T22:40:00+0000
-10000000000 minutes: -17006-12-01T01:20:00+0000
+10000000000 hours: 1142802-09-30T04:00:00+0000
-10000000000 hours: -1138787-07-28T20:00:00+0000
+10000000000 days: 27381078-03-25T12:00:00+0000
-10000000000 days: -27377062-02-02T12:00:00+0000
+10000000000 months: 833335341-06-28T12:00:00+0000
-10000000000 months: -833331326-10-28T12:00:00+0000
+10000000000 years: 10000002008-02-28T12:00:00+0000
-10000000000 years: -9999997992-02-28T12:00:00+0000"##,
    );
}

/// ext/date/tests/date_interval_create_from_date_string_nullparam.phpt (partial: the
/// deprecation notice is not expressible)
#[test]
fn date_interval_create_from_date_string_nullparam() {
    assert!(php::date_interval_create_from_date_string("").is_none());
    // "Unknown or bad format () at position 0 ( ): Empty string"
    let r = php_timelib::strtotime("", &builtin_tzinfo);
    assert_eq!(r.errors.errors[0].position, 0);
    assert_eq!(r.errors.errors[0].message.to_string(), "Empty string");
}

/// ext/date/tests/date_modify-1.phpt
#[test]
fn date_modify_1() {
    let mut out = String::new();
    let mut ts = DT::new("Fri Aug 20 1993 23:59:59", "Pacific/Kwajalein");
    let _ = writeln!(out, "{}", ts.format("D, d M Y H:i:s T"));
    assert!(ts.modify("+1 second"));
    let _ = writeln!(out, "{}", ts.format("D, d M Y H:i:s T"));

    let zone = "Europe/Amsterdam";
    let mut ts = DT::new("Sun Mar 27 01:59:59 2005", zone);
    let _ = writeln!(out, "{}", ts.format("D, d M Y H:i:s T"));
    assert!(ts.modify("+1 second"));
    let _ = writeln!(out, "{}", ts.format("D, d M Y H:i:s T"));

    let mut ts = DT::new("Sun Oct 30 01:59:59 2005", zone);
    let _ = writeln!(out, "{}", ts.format("D, d M Y H:i:s T"));
    assert!(ts.modify("+ 1 hour 1 second"));
    let _ = writeln!(out, "{}", ts.format("D, d M Y H:i:s T"));
    assert_text(
        &out,
        r##"Fri, 20 Aug 1993 23:59:59 -12
Sun, 22 Aug 1993 00:00:00 +12
Sun, 27 Mar 2005 01:59:59 CET
Sun, 27 Mar 2005 03:00:00 CEST
Sun, 30 Oct 2005 01:59:59 CEST
Sun, 30 Oct 2005 03:00:00 CET"##,
    );
}

/// ext/date/tests/date_parse_error.phpt
#[test]
fn date_parse_error() {
    let mut out = String::from("*** Testing date_parse() : error conditions ***\n\n");
    out.push_str(
        "-- Testing date_parse() function with unexpected characters in $date argument --\n",
    );
    out.push_str(&date_parse("2OO9-02--27 10:00?00.5").var_dump());
    assert_text(
        &out,
        r##"*** Testing date_parse() : error conditions ***

-- Testing date_parse() function with unexpected characters in $date argument --
array(13) {
  ["year"]=>
  bool(false)
  ["month"]=>
  bool(false)
  ["day"]=>
  bool(false)
  ["hour"]=>
  int(10)
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
    [4]=>
    string(29) "Double timezone specification"
  }
  ["error_count"]=>
  int(7)
  ["errors"]=>
  array(7) {
    [0]=>
    string(20) "Unexpected character"
    [1]=>
    string(47) "The timezone could not be found in the database"
    [3]=>
    string(20) "Unexpected character"
    [7]=>
    string(20) "Unexpected character"
    [8]=>
    string(29) "Double timezone specification"
    [17]=>
    string(20) "Unexpected character"
    [18]=>
    string(25) "Double time specification"
  }
  ["is_localtime"]=>
  bool(true)
  ["zone_type"]=>
  int(0)
}"##,
    );
}

/// ext/date/tests/date_sun_info_003.phpt
#[test]
fn date_sun_info_003() {
    let zone = "America/Sao_Paulo";
    let tzi = tz(zone);
    let mut out = String::new();
    for (n, d) in ["2015-01-12 00:00:00 UTC", "2015-09-12 00:00:00 UTC"]
        .iter()
        .enumerate()
    {
        if n > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "{d}");
        let si = php_timelib::sun_info(strtotime_abs(d, zone).unwrap(), 89.00, 1.00, &tzi);
        let v = |x: SunValue| match x {
            SunValue::Always(true) => "always".to_string(),
            SunValue::Always(false) => "never".to_string(),
            SunValue::Time(t) => date("H:i:s", t, zone),
        };
        let _ = writeln!(out, "sunrise: {}", v(si.sunrise));
        let _ = writeln!(out, "sunset: {}", v(si.sunset));
        let _ = writeln!(out, "transit: {}", date("H:i:s", si.transit, zone));
        let _ = writeln!(out, "civil_twilight_begin: {}", v(si.civil_twilight_begin));
        let _ = writeln!(out, "civil_twilight_end: {}", v(si.civil_twilight_end));
        let _ = writeln!(
            out,
            "nautical_twilight_begin: {}",
            v(si.nautical_twilight_begin)
        );
        let _ = writeln!(
            out,
            "nautical_twilight_end: {}",
            v(si.nautical_twilight_end)
        );
        let _ = writeln!(
            out,
            "astronomical_twilight_begin: {}",
            v(si.astronomical_twilight_begin)
        );
        let _ = writeln!(
            out,
            "astronomical_twilight_end: {}",
            v(si.astronomical_twilight_end)
        );
    }
    assert_text(
        &out,
        r##"2015-01-12 00:00:00 UTC
sunrise: never
sunset: never
transit: 10:03:48
civil_twilight_begin: never
civil_twilight_end: never
nautical_twilight_begin: never
nautical_twilight_end: never
astronomical_twilight_begin: never
astronomical_twilight_end: never

2015-09-12 00:00:00 UTC
sunrise: always
sunset: always
transit: 08:52:44
civil_twilight_begin: always
civil_twilight_end: always
nautical_twilight_begin: always
nautical_twilight_end: always
astronomical_twilight_begin: always
astronomical_twilight_end: always"##,
    );
}

/// ext/date/tests/date_sunset_variation8.phpt
#[test]
fn date_sunset_variation8() {
    let zone = "Africa/Casablanca";
    let tzi = tz(zone);
    let time_date = [
        ("12 Aug 2008", mktime(8, 8, 8, 8, 12, 2008, zone)),
        ("13 Aug 2008", mktime(8, 8, 8, 8, 13, 2008, zone)),
        ("12 Nov 2008", mktime(8, 8, 8, 11, 12, 2008, zone)),
        ("13 Nov 2008", mktime(8, 8, 8, 11, 13, 2008, zone)),
    ];
    let mut out = String::from("*** Testing date_sunset() : usage variation ***\n");
    for (d, time) in time_date {
        let _ = writeln!(out, "\n--{d}--");
        for lat in [90.0, -90.0] {
            for f in [SunFormat::String, SunFormat::Double, SunFormat::Timestamp] {
                let r = php_timelib::sunrise_sunset(time, f, lat, 0.0, 90.833333, None, &tzi, true);
                let _ = writeln!(
                    out,
                    "{}",
                    if r.is_none() {
                        "bool(false)".to_string()
                    } else {
                        format!("{r:?}")
                    }
                );
            }
        }
    }
    assert_text(
        &out,
        r##"*** Testing date_sunset() : usage variation ***

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
bool(false)"##,
    );
}

/// ext/date/tests/date_time_immutable001.phpt
#[test]
fn date_time_immutable001() {
    assert!(dt("Invalid", "UTC").is_none());
}

/// ext/date/tests/date_timezone_set_basic1.phpt
#[test]
fn date_timezone_set_basic1() {
    let zone = "Europe/London";
    let mut out = String::from("*** Testing date_timezone_set() : basic functionality ***\n");
    let datetime = DT::new("2009-01-30 17:57:32", zone);
    let _ = writeln!(out, "Default timezone: {}", datetime.timezone_name());
    let mut datetime = DT::new("2009-01-30 22:57:32", zone);
    datetime.set_timezone("America/Los_Angeles");
    let _ = writeln!(out, "New timezone: {}", datetime.timezone_name());
    assert_text(
        &out,
        r##"*** Testing date_timezone_set() : basic functionality ***
Default timezone: Europe/London
New timezone: America/Los_Angeles"##,
    );
}

/// `getdate($ts)` with `date_default_timezone_set($zone)`.
fn getdate(timestamp: i64, zone: &str) -> V {
    let mut t = Time {
        tz_info: Some(tz(zone)),
        zone_type: php_timelib::ZoneType::Id,
        ..Time::new()
    };
    t.unixtime2local(timestamp);
    V::Arr(vec![
        (K::S("seconds".into()), V::Int(t.s)),
        (K::S("minutes".into()), V::Int(t.i)),
        (K::S("hours".into()), V::Int(t.h)),
        (K::S("mday".into()), V::Int(t.d)),
        (
            K::S("wday".into()),
            V::Int(php_timelib::day_of_week(t.y, t.m, t.d)),
        ),
        (K::S("mon".into()), V::Int(t.m)),
        (K::S("year".into()), V::Int(t.y)),
        (
            K::S("yday".into()),
            V::Int(php_timelib::day_of_year(t.y, t.m, t.d)),
        ),
        (K::S("weekday".into()), V::Str(date("l", timestamp, zone))),
        (K::S("month".into()), V::Str(date("F", timestamp, zone))),
        (K::I(0), V::Int(timestamp)),
    ])
}

/// ext/date/tests/getdate_basic.phpt (`getdate()` without a timestamp uses a fixed
/// "now")
#[test]
fn getdate_basic() {
    let zone = "Asia/Calcutta";
    let mut out = String::from("*** Testing getdate() : basic functionality ***\n");
    out.push_str(&getdate(10, zone).var_dump());
    out.push_str(&getdate(NOW, zone).var_dump());
    assert_textf(
        &out,
        r##"*** Testing getdate() : basic functionality ***
array(11) {
  ["seconds"]=>
  int(10)
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
  int(10)
}
array(11) {
  ["seconds"]=>
  int(%d)
  ["minutes"]=>
  int(%d)
  ["hours"]=>
  int(%d)
  ["mday"]=>
  int(%d)
  ["wday"]=>
  int(%d)
  ["mon"]=>
  int(%d)
  ["year"]=>
  int(%d)
  ["yday"]=>
  int(%d)
  ["weekday"]=>
  string(%d) %s
  ["month"]=>
  string(%d) %s
  [0]=>
  int(%d)
}"##,
    );
    assert_eq!(
        getdate(NOW, zone).print_r().lines().nth(10).unwrap().trim(),
        "[weekday] => Friday"
    );
}

/// ext/date/tests/gh-124.phpt
#[test]
fn gh_124() {
    let ts = |s: &str| DT::new(s, "UTC").timestamp();
    let mut out = String::new();
    out.push_str(&vd_int(ts("@-9223372036854775808")));
    out.push_str(&vd_int(ts("-292277022657-01-27 08:29:52 UTC")));
    out.push_str(&vd_int(ts("-292277022657-01-27 08:29:53 UTC")));
    out.push_str(&vd_int(ts("@9223372036854775807")));
    out.push_str(&vd_int(ts("+292277026596-12-04 15:30:07 UTC")));
    out.push_str(&DT::new("+2512370164-01-01 00:00:00Z", "UTC").format(DATE_RFC3339_EXTENDED));
    assert_text(
        &out,
        r##"int(-9223372036854775808)
int(-9223372036854775808)
int(-9223372036854775807)
int(9223372036854775807)
int(9223372036854775807)
2512370164-01-01T00:00:00.000+00:00"##,
    );
}

/// ext/date/tests/gh11854.phpt
#[test]
fn gh11854() {
    let d = DT::from_format("D M d H:i:s Y", "Wed Aug  2 08:37:50 2023", None, "UTC").unwrap();
    assert_textf(
        &d.var_dump("DateTime", 1),
        r##"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-08-02 08:37:50.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}"##,
    );
}

/// ext/date/tests/gh18422.phpt
#[test]
fn gh18422() {
    let mut dto = DT::new("2006-12-12", "UTC");
    dto.set_isodate(i64::MIN, 1, 1);
    let mut out = String::new();
    let _ = writeln!(out, "{}", dto.format("Y"));
    let _ = writeln!(out, "{}", dto.format("x"));
    let _ = writeln!(out, "{}", dto.format("X"));
    let _ = writeln!(out, "{}", DT::new("2024-06-15", "UTC").format("Y"));
    let _ = writeln!(out, "{}", DT::new("-0042-01-01", "UTC").format("Y"));
    assert_textf(
        &out,
        r##"-%d
-%d
-%d
2024
-0042"##,
    );
}

/// ext/date/tests/gh9699.phpt
#[test]
fn gh9699() {
    let date = DT::new_in("2022-10-09 02:41:54.515330", "America/Los_Angeles", "UTC");
    let now = DT::new_in("2022-10-10 08:41:54.534620", "UTC", "UTC");
    assert_eq!(
        interval_format(&date.diff(&now), "%R %Y %M %D %H %I %S %F"),
        "+ 00 00 00 23 00 00 019290"
    );
}

/// ext/date/tests/gmdate_variation10.phpt (the calls without a timestamp use a fixed
/// "now"; their output does not depend on it)
#[test]
fn gmdate_variation10() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "Asia/Calcutta");
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    for (title, f) in [
        ("Timezone identifier format", "e"),
        (
            "checking whether date is in daylight saving time format",
            "I",
        ),
        ("difference to GMT in hours format", "O"),
        (
            "Difference to GMT in hours using colon as separator format",
            "P",
        ),
        ("timezone abbreviation format", "T"),
        ("timezone offset format", "T"),
    ] {
        let _ = writeln!(out, "\n-- Testing gmdate() function with {title} --");
        out.push_str(&vd_str(&gmdate(f, NOW)));
        out.push_str(&vd_str(&gmdate(f, timestamp)));
    }
    assert_textf(
        &out,
        r##"*** Testing gmdate() : usage variation ***

-- Testing gmdate() function with Timezone identifier format --
string(3) "UTC"
string(3) "UTC"

-- Testing gmdate() function with checking whether date is in daylight saving time format --
string(1) "%d"
string(1) "%d"

-- Testing gmdate() function with difference to GMT in hours format --
string(5) "+0000"
string(5) "+0000"

-- Testing gmdate() function with Difference to GMT in hours using colon as separator format --
string(6) "+00:00"
string(6) "+00:00"

-- Testing gmdate() function with timezone abbreviation format --
string(3) "GMT"
string(3) "GMT"

-- Testing gmdate() function with timezone offset format --
string(3) "GMT"
string(3) "GMT""##,
    );
}

/// ext/date/tests/gmdate_variation4.phpt (the calls without a timestamp use a fixed
/// "now")
#[test]
fn gmdate_variation4() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    for (title, f) in [
        ("partial textual representation of day", "D"),
        ("full textual representation of day", "l"),
        ("English ordinal suffix", "S"),
    ] {
        let _ = writeln!(out, "\n-- Testing gmdate() function with {title} --");
        out.push_str(&vd_str(&gmdate(f, NOW)));
        out.push_str(&vd_str(&gmdate(f, timestamp)));
    }
    assert_textf(
        &out,
        r##"*** Testing gmdate() : usage variation ***

-- Testing gmdate() function with partial textual representation of day --
string(%d) "%s"
string(3) "Fri"

-- Testing gmdate() function with full textual representation of day --
string(%d) "%s"
string(6) "Friday"

-- Testing gmdate() function with English ordinal suffix --
string(%d) "%s"
string(2) "th""##,
    );
}

/// ext/date/tests/gmmktime_variation7.phpt (with a fixed "now", 2005-07-14 22:30:41 UTC)
#[test]
fn gmmktime_variation7() {
    let g = |h, i: Option<i64>, s: Option<i64>, m: Option<i64>, d: Option<i64>| {
        php::php_mktime(h, i, s, m, d, None, NOW, None).unwrap()
    };
    let r = [
        g(8, None, None, None, None),
        g(8, Some(8), None, None, None),
        g(8, Some(8), Some(8), None, None),
        g(8, Some(8), Some(8), Some(8), None),
        g(8, Some(8), Some(8), Some(8), Some(8)),
    ];
    let mut out = String::from("*** Testing gmmktime() : usage variation ***\n");
    for (title, v) in ["one", "two", "three", "four", "five"].iter().zip(r) {
        let _ = writeln!(
            out,
            "\n-- Testing gmmktime() function with {title} optional argument --"
        );
        out.push_str(&vd_int(v));
    }
    assert_textf(
        &out,
        r##"*** Testing gmmktime() : usage variation ***

-- Testing gmmktime() function with one optional argument --
int(%d)

-- Testing gmmktime() function with two optional argument --
int(%d)

-- Testing gmmktime() function with three optional argument --
int(%d)

-- Testing gmmktime() function with four optional argument --
int(%d)

-- Testing gmmktime() function with five optional argument --
int(%d)"##,
    );
    let s: Vec<String> = r.iter().map(|&t| gmdate("Y-m-d H:i:s", t)).collect();
    assert_eq!(
        s,
        [
            "2005-07-14 08:30:41",
            "2005-07-14 08:08:41",
            "2005-07-14 08:08:08",
            "2005-08-14 08:08:08",
            "2005-08-08 08:08:08"
        ]
    );
}

/// ext/date/tests/idate_variation6.phpt
#[test]
fn idate_variation6() {
    let zone = "Asia/Calcutta";
    let mut out = String::from("*** Testing idate() : usage variation ***\n");
    out.push_str(
        "\n-- Testing idate() function for 2 digit year having no zero as starting number --\n",
    );
    let ts = mktime(8, 8, 8, 8, 8, 1970, zone);
    out.push_str(&vd_int(i64::from(idate('y', ts, zone).unwrap())));
    out.push_str(
        "\n-- Testing idate() function for 2 digit year having zero as starting number --\n",
    );
    let ts = mktime(8, 8, 8, 8, 8, 2001, zone);
    out.push_str(&vd_int(i64::from(idate('y', ts, zone).unwrap())));
    assert_text(
        &out,
        r##"*** Testing idate() : usage variation ***

-- Testing idate() function for 2 digit year having no zero as starting number --
int(70)

-- Testing idate() function for 2 digit year having zero as starting number --
int(1)"##,
    );
}

/// ext/date/tests/localtime_variation5.phpt
#[test]
fn localtime_variation5() {
    let mut out = String::from("*** Testing localtime() : usage variation ***\n");
    for (key, value) in [
        ("Hexa-decimal 0", 0x0),
        ("Hexa-decimal 10", 0xA),
        ("Hexa-decimal -10", -0xA),
    ] {
        let _ = writeln!(out, "\n--{key}--");
        let mut t = Time {
            tz_info: Some(tz("UTC")),
            zone_type: php_timelib::ZoneType::Id,
            ..Time::new()
        };
        t.unixtime2local(value);
        let vals = [
            ("tm_sec", t.s),
            ("tm_min", t.i),
            ("tm_hour", t.h),
            ("tm_mday", t.d),
            ("tm_mon", t.m - 1),
            ("tm_year", t.y - 1900),
            ("tm_wday", php_timelib::day_of_week(t.y, t.m, t.d)),
            ("tm_yday", php_timelib::day_of_year(t.y, t.m, t.d)),
            ("tm_isdst", i64::from(t.dst)),
        ];
        let indexed = V::Arr(
            vals.iter()
                .enumerate()
                .map(|(i, &(_, v))| (K::I(i as i64), V::Int(v)))
                .collect(),
        );
        let assoc = V::Arr(
            vals.iter()
                .map(|&(k, v)| (K::S(k.into()), V::Int(v)))
                .collect(),
        );
        out.push_str(&indexed.var_dump());
        out.push_str(&assoc.var_dump());
    }
    assert_text(
        &out,
        r##"*** Testing localtime() : usage variation ***

--Hexa-decimal 0--
array(9) {
  [0]=>
  int(0)
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
}
array(9) {
  ["tm_sec"]=>
  int(0)
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

--Hexa-decimal 10--
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
}
array(9) {
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

--Hexa-decimal -10--
array(9) {
  [0]=>
  int(50)
  [1]=>
  int(59)
  [2]=>
  int(23)
  [3]=>
  int(31)
  [4]=>
  int(11)
  [5]=>
  int(69)
  [6]=>
  int(3)
  [7]=>
  int(364)
  [8]=>
  int(0)
}
array(9) {
  ["tm_sec"]=>
  int(50)
  ["tm_min"]=>
  int(59)
  ["tm_hour"]=>
  int(23)
  ["tm_mday"]=>
  int(31)
  ["tm_mon"]=>
  int(11)
  ["tm_year"]=>
  int(69)
  ["tm_wday"]=>
  int(3)
  ["tm_yday"]=>
  int(364)
  ["tm_isdst"]=>
  int(0)
}"##,
    );
}

/// ext/date/tests/oom_date_interval_create_from_date_string.phpt
#[test]
fn oom_date_interval_create_from_date_string() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    assert!(php::date_interval_create_from_date_string(&s).is_none());
    // "Unknown or bad format (%s) at position 0 (%s): Unexpected character"
    let r = php_timelib::strtotime(&s, &builtin_tzinfo);
    assert_eq!(r.errors.errors[0].position, 0);
    assert_eq!(
        r.errors.errors[0].message.to_string(),
        "Unexpected character"
    );
}

/// ext/date/tests/oom_datetime_construct.phpt
#[test]
fn oom_datetime_construct() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    // "Failed to parse time string (%s) at position 0 (%s): Unexpected character"
    let e = DT::try_new(&s, None, "UTC").unwrap_err();
    assert_eq!(e.errors[0].position, 0);
    assert_eq!(e.errors[0].message.to_string(), "Unexpected character");
}

/// The RFC "DateTime and Daylight Saving Time Transitions" scripts.
struct Rfc {
    zone: &'static str,
    out: String,
}

impl Rfc {
    const DATE_FORMAT: &'static str = "Y-m-d H:i:s T e";
    const INTERVAL_FORMAT: &'static str = "P%dDT%hH";

    fn new() -> Rfc {
        Rfc {
            zone: "America/New_York",
            out: String::new(),
        }
    }

    fn diff(&mut self, label: &str, end: &str, start: &str, f: &str) {
        let end = DT::new(end, self.zone);
        let start = DT::new(start, self.zone);
        let _ = writeln!(
            self.out,
            "{label} {} - {} = {}",
            end.format(Self::DATE_FORMAT),
            start.format(Self::DATE_FORMAT),
            interval_format(&start.diff(&end), f)
        );
    }

    fn add(&mut self, label: &str, start: &str, spec: &str) {
        let mut start = DT::new(start, self.zone);
        let before = start.format(Self::DATE_FORMAT);
        start.add(spec);
        let _ = writeln!(
            self.out,
            "{label} {before} + {spec} = {}",
            start.format(Self::DATE_FORMAT)
        );
    }

    fn sub(&mut self, label: &str, end: &str, spec: &str) {
        let mut end = DT::new(end, self.zone);
        let before = end.format(Self::DATE_FORMAT);
        end.sub(spec);
        let _ = writeln!(
            self.out,
            "{label} {before} - {spec} = {}",
            end.format(Self::DATE_FORMAT)
        );
    }

    fn nl(&mut self) {
        self.out.push('\n');
    }
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type2.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type2() {
    let mut r = Rfc::new();
    let i = Rfc::INTERVAL_FORMAT;
    r.diff(
        "fd1",
        "2010-03-14 03:00:00 EDT",
        "2010-03-14 01:59:59 EST",
        "PT%hH%iM%sS",
    );
    r.diff(
        "fd2",
        "2010-03-14 04:30:00 EDT",
        "2010-03-13 04:30:00 EST",
        i,
    );
    r.diff(
        "fd3",
        "2010-03-14 03:30:00 EDT",
        "2010-03-13 04:30:00 EST",
        i,
    );
    r.diff(
        "fd4",
        "2010-03-14 01:30:00 EST",
        "2010-03-13 04:30:00 EST",
        i,
    );
    r.diff(
        "fd5",
        "2010-03-14 01:30:00 EST",
        "2010-03-13 01:30:00 EST",
        i,
    );
    r.diff(
        "fd6",
        "2010-03-14 03:30:00 EDT",
        "2010-03-13 03:30:00 EST",
        i,
    );
    r.diff(
        "fd7",
        "2010-03-14 03:30:00 EDT",
        "2010-03-13 02:30:00 EST",
        i,
    );
    r.nl();
    r.add("fa1", "2010-03-14 01:59:59 EST", "PT1S");
    r.add("fa2", "2010-03-13 04:30:00 EST", "P1D");
    r.add("fa3", "2010-03-13 04:30:00 EST", "PT22H");
    r.add("fa4", "2010-03-13 04:30:00 EST", "PT21H");
    r.add("fa5", "2010-03-13 01:30:00 EST", "P1D");
    r.add("fa6", "2010-03-13 02:30:00 EST", "P1D");
    r.nl();
    r.sub("fs1", "2010-03-14 03:00:00 EDT", "PT1S");
    r.sub("fs2", "2010-03-14 04:30:00 EDT", "P1D");
    r.sub("fs3", "2010-03-14 03:30:00 EDT", "PT22H");
    r.sub("fs4", "2010-03-14 01:30:00 EST", "PT21H");
    r.sub("fs5", "2010-03-14 01:30:00 EST", "P1D");
    r.sub("fs6", "2010-03-15 03:30:00 EDT", "P1D");
    r.sub("fs7", "2010-03-15 02:30:00 EDT", "P1D");
    r.nl();
    r.diff(
        "bd1",
        "2010-11-07 01:00:00 EST",
        "2010-11-07 01:59:59 EDT",
        "PT%hH%iM%sS",
    );
    r.diff(
        "bd2",
        "2010-11-07 04:30:00 EST",
        "2010-11-06 04:30:00 EDT",
        i,
    );
    r.diff(
        "bd3",
        "2010-11-07 03:30:00 EST",
        "2010-11-06 04:30:00 EDT",
        i,
    );
    r.diff(
        "bd4",
        "2010-11-07 02:30:00 EST",
        "2010-11-06 04:30:00 EDT",
        i,
    );
    r.diff(
        "bd5",
        "2010-11-07 01:30:00 EST",
        "2010-11-06 04:30:00 EDT",
        i,
    );
    r.diff(
        "bd6",
        "2010-11-07 01:30:00 EDT",
        "2010-11-06 04:30:00 EDT",
        i,
    );
    r.diff(
        "bd7",
        "2010-11-07 01:30:00 EDT",
        "2010-11-06 01:30:00 EDT",
        i,
    );
    r.diff(
        "bd8",
        "2010-11-07 01:30:00 EST",
        "2010-11-06 01:30:00 EDT",
        i,
    );
    r.nl();
    r.add("ba1", "2010-11-07 01:59:59 EDT", "PT1S");
    r.add("ba2", "2010-11-06 04:30:00 EDT", "P1D");
    r.add("ba3", "2010-11-06 04:30:00 EDT", "PT24H");
    r.add("ba4", "2010-11-06 04:30:00 EDT", "PT23H");
    r.add("ba5", "2010-11-06 04:30:00 EDT", "PT22H");
    r.add("ba6", "2010-11-06 04:30:00 EDT", "PT21H");
    r.add("ba7", "2010-11-06 01:30:00 EDT", "P1D");
    r.add("ba8", "2010-11-06 01:30:00 EDT", "P1DT1H");
    r.add("ba9", "2010-11-06 04:30:00 EDT", "PT25H");
    r.add("ba10", "2010-11-06 03:30:00 EDT", "P1D");
    r.add("ba11", "2010-11-06 02:30:00 EDT", "P1D");
    r.nl();
    r.sub("bs1", "2010-11-07 01:00:00 EST", "PT1S");
    r.sub("bs2", "2010-11-07 04:30:00 EST", "P1D");
    r.sub("bs3", "2010-11-07 03:30:00 EST", "PT24H");
    r.sub("bs4", "2010-11-07 02:30:00 EST", "PT23H");
    r.sub("bs5", "2010-11-07 01:30:00 EST", "PT22H");
    r.sub("bs6", "2010-11-07 01:30:00 EDT", "PT21H");
    r.sub("bs7", "2010-11-07 01:30:00 EDT", "P1D");
    r.sub("bs8", "2010-11-07 01:30:00 EST", "P1DT1H");
    r.sub("bs9", "2010-11-07 03:30:00 EST", "P1D");
    r.sub("bs10", "2010-11-07 02:30:00 EST", "P1D");
    assert_text(
        &r.out,
        r##"fd1 2010-03-14 03:00:00 EDT EDT - 2010-03-14 01:59:59 EST EST = PT0H0M1S
fd2 2010-03-14 04:30:00 EDT EDT - 2010-03-13 04:30:00 EST EST = P0DT23H
fd3 2010-03-14 03:30:00 EDT EDT - 2010-03-13 04:30:00 EST EST = P0DT22H
fd4 2010-03-14 01:30:00 EST EST - 2010-03-13 04:30:00 EST EST = P0DT21H
fd5 2010-03-14 01:30:00 EST EST - 2010-03-13 01:30:00 EST EST = P1DT0H
fd6 2010-03-14 03:30:00 EDT EDT - 2010-03-13 03:30:00 EST EST = P0DT23H
fd7 2010-03-14 03:30:00 EDT EDT - 2010-03-13 02:30:00 EST EST = P1DT0H

fa1 2010-03-14 01:59:59 EST EST + PT1S = 2010-03-14 02:00:00 EST EST
fa2 2010-03-13 04:30:00 EST EST + P1D = 2010-03-14 04:30:00 EST EST
fa3 2010-03-13 04:30:00 EST EST + PT22H = 2010-03-14 02:30:00 EST EST
fa4 2010-03-13 04:30:00 EST EST + PT21H = 2010-03-14 01:30:00 EST EST
fa5 2010-03-13 01:30:00 EST EST + P1D = 2010-03-14 01:30:00 EST EST
fa6 2010-03-13 02:30:00 EST EST + P1D = 2010-03-14 02:30:00 EST EST

fs1 2010-03-14 03:00:00 EDT EDT - PT1S = 2010-03-14 02:59:59 EDT EDT
fs2 2010-03-14 04:30:00 EDT EDT - P1D = 2010-03-13 04:30:00 EDT EDT
fs3 2010-03-14 03:30:00 EDT EDT - PT22H = 2010-03-13 05:30:00 EDT EDT
fs4 2010-03-14 01:30:00 EST EST - PT21H = 2010-03-13 04:30:00 EST EST
fs5 2010-03-14 01:30:00 EST EST - P1D = 2010-03-13 01:30:00 EST EST
fs6 2010-03-15 03:30:00 EDT EDT - P1D = 2010-03-14 03:30:00 EDT EDT
fs7 2010-03-15 02:30:00 EDT EDT - P1D = 2010-03-14 02:30:00 EDT EDT

bd1 2010-11-07 01:00:00 EST EST - 2010-11-07 01:59:59 EDT EDT = PT0H0M1S
bd2 2010-11-07 04:30:00 EST EST - 2010-11-06 04:30:00 EDT EDT = P1DT1H
bd3 2010-11-07 03:30:00 EST EST - 2010-11-06 04:30:00 EDT EDT = P1DT0H
bd4 2010-11-07 02:30:00 EST EST - 2010-11-06 04:30:00 EDT EDT = P0DT23H
bd5 2010-11-07 01:30:00 EST EST - 2010-11-06 04:30:00 EDT EDT = P0DT22H
bd6 2010-11-07 01:30:00 EDT EDT - 2010-11-06 04:30:00 EDT EDT = P0DT21H
bd7 2010-11-07 01:30:00 EDT EDT - 2010-11-06 01:30:00 EDT EDT = P1DT0H
bd8 2010-11-07 01:30:00 EST EST - 2010-11-06 01:30:00 EDT EDT = P1DT1H

ba1 2010-11-07 01:59:59 EDT EDT + PT1S = 2010-11-07 02:00:00 EDT EDT
ba2 2010-11-06 04:30:00 EDT EDT + P1D = 2010-11-07 04:30:00 EDT EDT
ba3 2010-11-06 04:30:00 EDT EDT + PT24H = 2010-11-07 04:30:00 EDT EDT
ba4 2010-11-06 04:30:00 EDT EDT + PT23H = 2010-11-07 03:30:00 EDT EDT
ba5 2010-11-06 04:30:00 EDT EDT + PT22H = 2010-11-07 02:30:00 EDT EDT
ba6 2010-11-06 04:30:00 EDT EDT + PT21H = 2010-11-07 01:30:00 EDT EDT
ba7 2010-11-06 01:30:00 EDT EDT + P1D = 2010-11-07 01:30:00 EDT EDT
ba8 2010-11-06 01:30:00 EDT EDT + P1DT1H = 2010-11-07 02:30:00 EDT EDT
ba9 2010-11-06 04:30:00 EDT EDT + PT25H = 2010-11-07 05:30:00 EDT EDT
ba10 2010-11-06 03:30:00 EDT EDT + P1D = 2010-11-07 03:30:00 EDT EDT
ba11 2010-11-06 02:30:00 EDT EDT + P1D = 2010-11-07 02:30:00 EDT EDT

bs1 2010-11-07 01:00:00 EST EST - PT1S = 2010-11-07 00:59:59 EST EST
bs2 2010-11-07 04:30:00 EST EST - P1D = 2010-11-06 04:30:00 EST EST
bs3 2010-11-07 03:30:00 EST EST - PT24H = 2010-11-06 03:30:00 EST EST
bs4 2010-11-07 02:30:00 EST EST - PT23H = 2010-11-06 03:30:00 EST EST
bs5 2010-11-07 01:30:00 EST EST - PT22H = 2010-11-06 03:30:00 EST EST
bs6 2010-11-07 01:30:00 EDT EDT - PT21H = 2010-11-06 04:30:00 EDT EDT
bs7 2010-11-07 01:30:00 EDT EDT - P1D = 2010-11-06 01:30:00 EDT EDT
bs8 2010-11-07 01:30:00 EST EST - P1DT1H = 2010-11-06 00:30:00 EST EST
bs9 2010-11-07 03:30:00 EST EST - P1D = 2010-11-06 03:30:00 EST EST
bs10 2010-11-07 02:30:00 EST EST - P1D = 2010-11-06 02:30:00 EST EST"##,
    );
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type3-fd.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type3_fd() {
    let mut r = Rfc::new();
    let i = Rfc::INTERVAL_FORMAT;
    r.diff(
        "fd1",
        "2010-03-14 03:00:00",
        "2010-03-14 01:59:59",
        "PT%hH%iM%sS",
    );
    r.diff("fd2", "2010-03-14 04:30:00", "2010-03-13 04:30:00", i);
    r.diff("fd3", "2010-03-14 03:30:00", "2010-03-13 04:30:00", i);
    r.diff("fd4", "2010-03-14 01:30:00", "2010-03-13 04:30:00", i);
    r.diff("fd5", "2010-03-14 01:30:00", "2010-03-13 01:30:00", i);
    r.diff("fd6", "2010-03-14 03:30:00", "2010-03-13 03:30:00", i);
    r.diff("fd7", "2010-03-14 03:30:00", "2010-03-13 02:30:00", i);
    assert_text(
        &r.out,
        r##"fd1 2010-03-14 03:00:00 EDT America/New_York - 2010-03-14 01:59:59 EST America/New_York = PT0H0M1S
fd2 2010-03-14 04:30:00 EDT America/New_York - 2010-03-13 04:30:00 EST America/New_York = P1DT0H
fd3 2010-03-14 03:30:00 EDT America/New_York - 2010-03-13 04:30:00 EST America/New_York = P0DT22H
fd4 2010-03-14 01:30:00 EST America/New_York - 2010-03-13 04:30:00 EST America/New_York = P0DT21H
fd5 2010-03-14 01:30:00 EST America/New_York - 2010-03-13 01:30:00 EST America/New_York = P1DT0H
fd6 2010-03-14 03:30:00 EDT America/New_York - 2010-03-13 03:30:00 EST America/New_York = P1DT0H
fd7 2010-03-14 03:30:00 EDT America/New_York - 2010-03-13 02:30:00 EST America/New_York = P1DT1H"##,
    );
}

/// ext/date/tests/strtotime.phpt
#[test]
fn strtotime_phpt() {
    let zone = "Europe/Oslo";
    let mut out = String::new();
    for s in [
        "2005-07-14 22:30:41",
        "2005-07-14 22:30:41 GMT",
        "@1121373041",
        "@1121373041 CEST",
    ] {
        let _ = writeln!(
            out,
            "{}",
            date(DATE_ISO8601, strtotime_abs(s, zone).unwrap(), zone)
        );
    }
    assert_text(
        &out,
        r##"2005-07-14T22:30:41+0200
2005-07-15T00:30:41+0200
2005-07-14T22:30:41+0200
2005-07-14T22:30:41+0200"##,
    );
}

/// ext/date/tests/strtotime_variation_scottish.phpt (the date part comes from a fixed
/// "now" and is not printed)
#[test]
fn strtotime_variation_scottish() {
    let mut out = String::new();
    for s in ["back of 7", "front of 7", "back of 19", "front of 19"] {
        out.push_str(&vd_str(&date(
            "H:i:s",
            strtotime(s, NOW, "UTC").unwrap(),
            "UTC",
        )));
    }
    assert_text(
        &out,
        r##"string(8) "07:15:00"
string(8) "06:45:00"
string(8) "19:15:00"
string(8) "18:45:00""##,
    );
}

/// ext/date/tests/timezone-configuration.phpt
#[test]
fn timezone_configuration() {
    let mut out = String::new();
    for zone in ["Europe/Oslo", "Europe/London", "Europe/Oslo"] {
        let _ = writeln!(
            out,
            "{}",
            strtotime_abs("2005-06-18 22:15:44", zone).unwrap()
        );
    }
    assert_text(
        &out,
        r##"1119125744
1119129344
1119125744"##,
    );
}

/// ext/date/tests/timezone_offset_get_basic1.phpt (`date_create("GMT")` uses a fixed
/// "now"; the expected output accepts both the summer and the winter offset)
#[test]
fn timezone_offset_get_basic1() {
    let date = DT::new("GMT", "GMT");
    let mut offsets = Vec::new();
    for zone in ["Europe/London", "America/New_York", "America/Los_Angeles"] {
        offsets.push(tz(zone).get_time_zone_info(date.timestamp()).offset);
    }
    assert!([0, 3600].contains(&offsets[0]), "{offsets:?}");
    assert!([-18000, -14400].contains(&offsets[1]), "{offsets:?}");
    assert!([-28800, -25200].contains(&offsets[2]), "{offsets:?}");
    // NOW is in July.
    assert_eq!(offsets, [3600, -14400, -25200]);
}

/// ext/date/tests/timezone_version_get_basic1.phpt
#[test]
fn timezone_version_get_basic1() {
    // ^[12][0-9]{3}.[0-9]+$
    let v = builtin_db().version().as_bytes();
    assert!(v.len() >= 6, "{v:?}");
    assert!(matches!(v[0], b'1' | b'2'), "{v:?}");
    assert!(v[1..4].iter().all(u8::is_ascii_digit), "{v:?}");
    assert!(v[5..].iter().all(u8::is_ascii_digit), "{v:?}");
}

// ---------------------------------------------------------------------------------
// Data from DateTime_data-spring-type3-type2.inc, DateTime_data-fall-type3-type2.inc and
// DateTime_data-february.inc.

const SPRING_TYPE3_TYPE2: &[Case] = &[
    Case {
        name: "test_time_spring_type3_prev_type2_prev",
        end: "2010-03-13 18:38:28 EST",
        end_tz: false,
        start: "2010-02-11 02:18:48",
        start_tz: false,
        spec: "P+0Y1M2DT16H19M40S",
    },
    Case {
        name: "test_time_spring_type3_prev_type2_st",
        end: "2010-03-14 00:10:20 EST",
        end_tz: false,
        start: "2010-03-13 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT5H31M52S",
    },
    Case {
        name: "test_time_spring_type3_prev_type2_dt",
        end: "2010-03-14 03:16:55 EDT",
        end_tz: false,
        start: "2010-03-13 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT7H38M27S",
    },
    Case {
        name: "test_time_spring_type3_prev_type2_post",
        end: "2010-03-15 19:59:59 EDT",
        end_tz: false,
        start: "2010-03-13 18:38:28",
        start_tz: false,
        spec: "P+0Y0M2DT1H21M31S",
    },
    Case {
        name: "test_time_spring_type3_st_type2_prev",
        end: "2010-03-13 18:38:28 EST",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P-0Y0M0DT5H31M52S",
    },
    Case {
        name: "test_time_spring_type3_st_type2_st",
        end: "2010-03-14 00:15:35 EST",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT0H5M15S",
    },
    Case {
        name: "test_time_spring_type3_st_type2_dt",
        end: "2010-03-14 03:16:55 EDT",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT2H6M35S",
    },
    Case {
        name: "test_time_spring_type3_st_type2_post",
        end: "2010-03-15 19:59:59 EDT",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P+0Y0M1DT18H49M39S",
    },
    Case {
        name: "test_time_spring_type3_dt_type2_prev",
        end: "2010-03-13 18:38:28 EST",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT7H38M27S",
    },
    Case {
        name: "test_time_spring_type3_dt_type2_st",
        end: "2010-03-14 00:10:20 EST",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT2H6M35S",
    },
    Case {
        name: "test_time_spring_type3_dt_type2_dt",
        end: "2010-03-14 05:19:56 EDT",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P+0Y0M0DT2H3M1S",
    },
    Case {
        name: "test_time_spring_type3_dt_type2_post",
        end: "2010-03-15 19:59:59 EDT",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P+0Y0M1DT16H43M4S",
    },
    Case {
        name: "test_time_spring_type3_post_type2_prev",
        end: "2010-03-13 18:38:28 EST",
        end_tz: false,
        start: "2010-03-15 19:59:59",
        start_tz: false,
        spec: "P-0Y0M2DT1H21M31S",
    },
    Case {
        name: "test_time_spring_type3_post_type2_st",
        end: "2010-03-14 00:10:20 EST",
        end_tz: false,
        start: "2010-03-15 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT18H49M39S",
    },
    Case {
        name: "test_time_spring_type3_post_type2_dt",
        end: "2010-03-14 03:16:55 EDT",
        end_tz: false,
        start: "2010-03-15 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT16H43M4S",
    },
    Case {
        name: "test_time_spring_type3_post_type2_post",
        end: "2010-03-15 19:59:59 EDT",
        end_tz: false,
        start: "2010-03-15 18:57:55",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M4S",
    },
    Case {
        name: "test_time_spring_type3_stsec_type2_dtsec",
        end: "2010-03-14 03:00:00 EDT",
        end_tz: false,
        start: "2010-03-14 01:59:59",
        start_tz: false,
        spec: "P+0Y0M0DT0H0M1S",
    },
    Case {
        name: "test_time_spring_type3_dtsec_type2_stsec",
        end: "2010-03-14 01:59:59 EST",
        end_tz: false,
        start: "2010-03-14 03:00:00",
        start_tz: false,
        spec: "P-0Y0M0DT0H0M1S",
    },
];

const FALL_TYPE3_TYPE2: &[Case] = &[
    Case {
        name: "test_time_fall_type3_prev_type2_prev",
        end: "2010-11-06 18:38:28 EDT",
        end_tz: false,
        start: "2010-10-04 02:18:48",
        start_tz: false,
        spec: "P+0Y1M2DT16H19M40S",
    },
    Case {
        name: "test_time_fall_type3_prev_type2_dt",
        end: "2010-11-07 00:10:20 EDT",
        end_tz: false,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT5H31M52S",
    },
    Case {
        name: "test_time_fall_type3_prev_type2_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT6H34M5S",
    },
    Case {
        name: "test_time_fall_type3_prev_type2_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT7H36M16S",
    },
    Case {
        name: "test_time_fall_type3_prev_type2_st",
        end: "2010-11-07 03:16:55 EST",
        end_tz: false,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT9H38M27S",
    },
    Case {
        name: "test_time_fall_type3_prev_type2_post",
        end: "2010-11-08 19:59:59 EST",
        end_tz: false,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M2DT1H21M31S",
    },
    Case {
        name: "test_time_fall_type3_dt_type2_prev",
        end: "2010-11-06 18:38:28 EDT",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P-0Y0M0DT5H31M52S",
    },
    Case {
        name: "test_time_fall_type3_dt_type2_dt",
        end: "2010-11-07 00:15:35 EDT",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT0H5M15S",
    },
    Case {
        name: "test_time_fall_type3_dt_type2_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M13S",
    },
    Case {
        name: "test_time_fall_type3_dt_type2_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT2H4M24S",
    },
    Case {
        name: "test_time_fall_type3_dt_type2_st",
        end: "2010-11-07 03:16:55 EST",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT4H6M35S",
    },
    Case {
        name: "test_time_fall_type3_dt_type2_post",
        end: "2010-11-08 19:59:59 EST",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M1DT20H49M39S",
    },
    Case {
        name: "test_time_fall_type3_redodt_type2_prev",
        end: "2010-11-06 18:38:28 EDT",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P-0Y0M0DT6H34M5S",
    },
    Case {
        name: "test_time_fall_type3_redodt_type2_dt",
        end: "2010-11-07 00:10:20 EDT",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P-0Y0M0DT1H2M13S",
    },
    Case {
        name: "test_time_fall_type3_redodt_type2_redodt",
        end: "2010-11-07 01:15:35 EDT",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT0H3M2S",
    },
    Case {
        name: "test_time_fall_type3_redodt_type2_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT1H2M11S",
    },
    Case {
        name: "test_time_fall_type3_redodt_type2_st",
        end: "2010-11-07 03:16:55 EST",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT3H4M22S",
    },
    Case {
        name: "test_time_fall_type3_redodt_type2_post",
        end: "2010-11-08 19:59:59 EST",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M1DT19H47M26S",
    },
    Case {
        name: "test_time_fall_type3_redost_type2_prev",
        end: "2010-11-06 18:38:28 EDT",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P-0Y0M0DT7H36M16S",
    },
    Case {
        name: "test_time_fall_type3_redost_type2_dt",
        end: "2010-11-07 00:10:20 EDT",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P-0Y0M0DT2H4M24S",
    },
    Case {
        name: "test_time_fall_type3_redost_type2_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P-0Y0M0DT1H2M11S",
    },
    Case {
        name: "test_time_fall_type3_redost_type2_redost",
        end: "2010-11-07 01:16:54 EST",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P+0Y0M0DT0H2M10S",
    },
    Case {
        name: "test_time_fall_type3_redost_type2_st",
        end: "2010-11-07 03:16:55 EST",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P+0Y0M0DT2H2M11S",
    },
    Case {
        name: "test_time_fall_type3_redost_type2_post",
        end: "2010-11-08 19:59:59 EST",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P+0Y0M1DT18H45M15S",
    },
    Case {
        name: "test_time_fall_type3_st_type2_prev",
        end: "2010-11-06 18:38:28 EDT",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT9H38M27S",
    },
    Case {
        name: "test_time_fall_type3_st_type2_dt",
        end: "2010-11-07 00:10:20 EDT",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT4H6M35S",
    },
    Case {
        name: "test_time_fall_type3_st_type2_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT3H4M22S",
    },
    Case {
        name: "test_time_fall_type3_st_type2_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT2H2M11S",
    },
    Case {
        name: "test_time_fall_type3_st_type2_st",
        end: "2010-11-07 05:19:56 EST",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P+0Y0M0DT2H3M1S",
    },
    Case {
        name: "test_time_fall_type3_st_type2_post",
        end: "2010-11-08 19:59:59 EST",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P+0Y0M1DT16H43M4S",
    },
    Case {
        name: "test_time_fall_type3_post_type2_prev",
        end: "2010-11-06 18:38:28 EDT",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M2DT1H21M31S",
    },
    Case {
        name: "test_time_fall_type3_post_type2_dt",
        end: "2010-11-07 00:10:20 EDT",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT20H49M39S",
    },
    Case {
        name: "test_time_fall_type3_post_type2_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT19H47M26S",
    },
    Case {
        name: "test_time_fall_type3_post_type2_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT18H45M15S",
    },
    Case {
        name: "test_time_fall_type3_post_type2_st",
        end: "2010-11-07 03:16:55 EST",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT16H43M4S",
    },
    Case {
        name: "test_time_fall_type3_post_type2_post",
        end: "2010-11-08 19:59:59 EST",
        end_tz: false,
        start: "2010-11-08 18:57:55",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M4S",
    },
    Case {
        name: "test_time_fall_type3_dtsec_type2_stsec",
        end: "2010-11-07 01:00:00 EST",
        end_tz: false,
        start: "2010-11-07 01:59:59 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT0H0M1S",
    },
    Case {
        name: "test_time_fall_type3_stsec_type2_dtsec",
        end: "2010-11-07 01:59:59 EDT",
        end_tz: false,
        start: "2010-11-07 01:00:00 EST",
        start_tz: true,
        spec: "P-0Y0M0DT0H0M1S",
    },
];

const FEBRUARY: &[Case] = &[
    Case {
        name: "test_bug_49081__1",
        end: "2010-03-31",
        end_tz: false,
        start: "2010-03-01",
        start_tz: false,
        spec: "P+0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__2",
        end: "2010-04-01",
        end_tz: false,
        start: "2010-03-01",
        start_tz: false,
        spec: "P+0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__3",
        end: "2010-04-01",
        end_tz: false,
        start: "2010-03-31",
        start_tz: false,
        spec: "P+0Y0M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__4",
        end: "2010-04-29",
        end_tz: false,
        start: "2010-03-31",
        start_tz: false,
        spec: "P+0Y0M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__5",
        end: "2010-04-30",
        end_tz: false,
        start: "2010-03-31",
        start_tz: false,
        spec: "P+0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__6",
        end: "2010-04-30",
        end_tz: false,
        start: "2010-03-30",
        start_tz: false,
        spec: "P+0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__7",
        end: "2010-04-30",
        end_tz: false,
        start: "2010-03-29",
        start_tz: false,
        spec: "P+0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__8",
        end: "2010-01-29",
        end_tz: false,
        start: "2010-01-01",
        start_tz: false,
        spec: "P+0Y0M28DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__9",
        end: "2010-01-30",
        end_tz: false,
        start: "2010-01-01",
        start_tz: false,
        spec: "P+0Y0M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__10",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-01-01",
        start_tz: false,
        spec: "P+0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__11",
        end: "2010-02-01",
        end_tz: false,
        start: "2010-01-01",
        start_tz: false,
        spec: "P+0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__12",
        end: "2010-02-01",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y0M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__13",
        end: "2010-02-27",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y0M27DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__14",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y0M28DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__15",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-01-30",
        start_tz: false,
        spec: "P+0Y0M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__16",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-01-29",
        start_tz: false,
        spec: "P+0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__17",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-01-28",
        start_tz: false,
        spec: "P+0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__18",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-01-27",
        start_tz: false,
        spec: "P+0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__19",
        end: "2010-03-01",
        end_tz: false,
        start: "2010-01-01",
        start_tz: false,
        spec: "P+0Y2M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__20",
        end: "2010-03-01",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y0M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__21",
        end: "2010-03-27",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y1M24DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__22",
        end: "2010-03-28",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y1M25DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__23",
        end: "2010-03-29",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y1M26DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__24",
        end: "2010-03-30",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y1M27DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__25",
        end: "2010-03-31",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P+0Y2M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__26",
        end: "2010-03-31",
        end_tz: false,
        start: "2010-01-30",
        start_tz: false,
        spec: "P+0Y2M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__27",
        end: "2009-01-31",
        end_tz: false,
        start: "2009-01-01",
        start_tz: false,
        spec: "P+0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__28",
        end: "2010-03-27",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P+0Y0M27DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__29",
        end: "2010-03-28",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P+0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__30",
        end: "2010-03-29",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P+0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__31",
        end: "2010-03-27",
        end_tz: false,
        start: "2010-02-27",
        start_tz: false,
        spec: "P+0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081__32",
        end: "2010-03-27",
        end_tz: false,
        start: "2010-02-26",
        start_tz: false,
        spec: "P+0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__1",
        end: "2010-03-01",
        end_tz: false,
        start: "2010-03-31",
        start_tz: false,
        spec: "P-0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__2",
        end: "2010-03-01",
        end_tz: false,
        start: "2010-04-01",
        start_tz: false,
        spec: "P-0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__3",
        end: "2010-03-31",
        end_tz: false,
        start: "2010-04-01",
        start_tz: false,
        spec: "P-0Y0M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__4",
        end: "2010-03-31",
        end_tz: false,
        start: "2010-04-29",
        start_tz: false,
        spec: "P-0Y0M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__5",
        end: "2010-03-31",
        end_tz: false,
        start: "2010-04-30",
        start_tz: false,
        spec: "P-0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__6",
        end: "2010-03-30",
        end_tz: false,
        start: "2010-04-30",
        start_tz: false,
        spec: "P-0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__7",
        end: "2010-03-29",
        end_tz: false,
        start: "2010-04-30",
        start_tz: false,
        spec: "P-0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__8",
        end: "2010-01-01",
        end_tz: false,
        start: "2010-01-29",
        start_tz: false,
        spec: "P-0Y0M28DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__9",
        end: "2010-01-01",
        end_tz: false,
        start: "2010-01-30",
        start_tz: false,
        spec: "P-0Y0M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__10",
        end: "2010-01-01",
        end_tz: false,
        start: "2010-01-31",
        start_tz: false,
        spec: "P-0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__11",
        end: "2010-01-01",
        end_tz: false,
        start: "2010-02-01",
        start_tz: false,
        spec: "P-0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__12",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-02-01",
        start_tz: false,
        spec: "P-0Y0M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__13",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-02-27",
        start_tz: false,
        spec: "P-0Y0M27DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__14",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P-0Y0M28DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__15",
        end: "2010-01-30",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P-0Y0M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__16",
        end: "2010-01-29",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P-0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__17",
        end: "2010-01-28",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P-0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__18",
        end: "2010-01-27",
        end_tz: false,
        start: "2010-02-28",
        start_tz: false,
        spec: "P-0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__19",
        end: "2010-01-01",
        end_tz: false,
        start: "2010-03-01",
        start_tz: false,
        spec: "P-0Y2M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__20",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-03-01",
        start_tz: false,
        spec: "P-0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__21",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-03-27",
        start_tz: false,
        spec: "P-0Y1M27DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__22",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-03-28",
        start_tz: false,
        spec: "P-0Y1M28DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__23",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-03-29",
        start_tz: false,
        spec: "P-0Y1M29DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__24",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-03-30",
        start_tz: false,
        spec: "P-0Y1M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__25",
        end: "2010-01-31",
        end_tz: false,
        start: "2010-03-31",
        start_tz: false,
        spec: "P-0Y2M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__26",
        end: "2010-01-30",
        end_tz: false,
        start: "2010-03-31",
        start_tz: false,
        spec: "P-0Y2M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__27",
        end: "2009-01-01",
        end_tz: false,
        start: "2009-01-31",
        start_tz: false,
        spec: "P-0Y0M30DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__28",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-03-27",
        start_tz: false,
        spec: "P-0Y0M27DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__29",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-03-28",
        start_tz: false,
        spec: "P-0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__30",
        end: "2010-02-28",
        end_tz: false,
        start: "2010-03-29",
        start_tz: false,
        spec: "P-0Y1M1DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__31",
        end: "2010-02-27",
        end_tz: false,
        start: "2010-03-27",
        start_tz: false,
        spec: "P-0Y1M0DT0H0M0S",
    },
    Case {
        name: "test_bug_49081_negative__32",
        end: "2010-02-26",
        end_tz: false,
        start: "2010-03-27",
        start_tz: false,
        spec: "P-0Y1M1DT0H0M0S",
    },
];
