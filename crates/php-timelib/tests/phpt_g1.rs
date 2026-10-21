//! Tests ported from `ext/date/tests/*.phpt` (group 1).
//!
//! Each test names the phpt file it was ported from. Object handles (`#N`) in
//! `EXPECTF` sections are filled in with fixed numbers; parts that depend on the real
//! current time, serialization, exceptions or class inheritance are left out.
mod common;

use std::cmp::Ordering;
use std::fmt::Write as _;

use common::*;
use php_timelib::php::{self, PhpInterval, PhpTimeZone};
use php_timelib::{SunFormat, SunResult, SunValue, Time, ZoneType};

const DATE_ISO8601: &str = "Y-m-d\\TH:i:sO";
const DATE_RFC2822: &str = "D, d M Y H:i:s O";
const DATE_ATOM: &str = "Y-m-d\\TH:i:sP";
const DATE_W3C: &str = "Y-m-d\\TH:i:sP";

/// `var_dump()` of a string.
fn vd_str(s: &str) -> String {
    V::Str(s.to_string()).var_dump()
}

/// `var_dump()` of an int.
fn vd_int(i: i64) -> String {
    V::Int(i).var_dump()
}

/// `var_dump()` of a bool.
fn vd_bool(b: bool) -> String {
    V::Bool(b).var_dump()
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

/// `print_r($interval)`.
fn interval_print_r(iv: &PhpInterval) -> String {
    V::Obj("DateInterval".into(), 0, interval_props(iv)).print_r()
}

/// `DateTimeZone::getOffset($dt)` (`timezone_offset_get`).
fn timezone_offset_get(z: &PhpTimeZone, t: &Time) -> i64 {
    match z {
        PhpTimeZone::Id(tz) => i64::from(tz.get_time_zone_info(t.sse).offset),
        PhpTimeZone::Offset(o) => *o,
        PhpTimeZone::Abbr(a) => a.utc_offset + i64::from(a.dst) * 3600,
    }
}

/// `$a == $b` for `DateTimeZone` objects (`date_object_compare_timezone`): `Err` is
/// the exception message.
fn timezone_equals(a: &PhpTimeZone, b: &PhpTimeZone) -> Result<bool, &'static str> {
    if a.zone_type() != b.zone_type() {
        return Err("Cannot compare two different kinds of DateTimeZone objects");
    }
    Ok(match (a, b) {
        (PhpTimeZone::Offset(x), PhpTimeZone::Offset(y)) => x == y,
        _ => a.name() == b.name(),
    })
}

/// `date_period_advance()`.
fn period_advance(t: &mut Time, iv: &PhpInterval) {
    t.have_relative = true;
    t.relative = iv.diff;
    t.sse_uptodate = false;
    t.update_ts(None);
    t.update_from_sse();
}

/// The dates a `DatePeriod($start, $interval, $end, $options)` iterates over.
fn period_dates(start: &DT, iv: &PhpInterval, end: &DT, include_end: bool) -> Vec<DT> {
    let mut out = Vec::new();
    let mut cur = start.clone();
    loop {
        let more = if cur.t.sse == end.t.sse {
            if include_end {
                cur.t.us <= end.t.us
            } else {
                cur.t.us < end.t.us
            }
        } else {
            cur.t.sse < end.t.sse
        };
        if !more {
            break;
        }
        out.push(cur.clone());
        period_advance(&mut cur.t, iv);
    }
    out
}

/// ext/date/tests/003.phpt
#[test]
fn t_003() {
    let mut out = String::new();
    for i in 0..32 {
        out += &vd_str(&date("jS", mktime(0, 0, 0, 1, i, 2006, "UTC"), "UTC"));
    }
    out += "Done\n";
    assert_text(
        &out,
        r#"string(4) "31st"
string(3) "1st"
string(3) "2nd"
string(3) "3rd"
string(3) "4th"
string(3) "5th"
string(3) "6th"
string(3) "7th"
string(3) "8th"
string(3) "9th"
string(4) "10th"
string(4) "11th"
string(4) "12th"
string(4) "13th"
string(4) "14th"
string(4) "15th"
string(4) "16th"
string(4) "17th"
string(4) "18th"
string(4) "19th"
string(4) "20th"
string(4) "21st"
string(4) "22nd"
string(4) "23rd"
string(4) "24th"
string(4) "25th"
string(4) "26th"
string(4) "27th"
string(4) "28th"
string(4) "29th"
string(4) "30th"
string(4) "31st"
Done
"#,
    );
}

/// ext/date/tests/014.phpt (partial: the TypeError is not ported)
#[test]
fn t_014() {
    let dto = DT::new("2006-12-12", "UTC");
    let mut out = dto.var_dump("DateTime", 1);
    let dtz = PhpTimeZone::from_time(&dto.t).unwrap();
    out += &V::Obj("DateTimeZone".into(), 2, tz_props(&dtz)).var_dump();
    out += &vd_int(timezone_offset_get(&dtz, &dto.t));
    assert_text(
        &out,
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2006-12-12 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTimeZone)#2 (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
int(0)
"#,
    );
}

/// ext/date/tests/DateInterval_format.phpt
#[test]
fn dateinterval_format() {
    let z = "UTC";
    let date1 = DT::new("2000-01-01 00:00:00", z);
    let date2 = DT::new("2001-03-04 04:05:06", z);
    let interval = date1.diff(&date2);
    let mut out = String::new();
    for f in [
        "Y=%Y", "M=%M", "D=%D", "H=%H", "I=%I", "S=%S", "R=%R", "y=%y", "m=%m", "d=%d", "h=%h",
        "i=%i", "s=%s", "r=%r",
    ] {
        out += &interval_format(&interval, f);
        out += "\n";
    }
    out += "\n";
    let interval = date2.diff(&date1);
    out += &interval_format(&interval, "inverted R=%R");
    out += "\n";
    out += &interval_format(&interval, "inverted r=%r");
    out += "\n\n";
    out += &interval_format(&interval, "%=%%");
    out += "\n";
    out += &interval_format(&interval, "x=%x");
    out += "\n";
    assert_text(
        &out,
        "Y=01
M=02
D=03
H=04
I=05
S=06
R=+
y=1
m=2
d=3
h=4
i=5
s=6
r=

inverted R=-
inverted r=-

%=%
x=%x
",
    );
}

/// ext/date/tests/DatePeriod_getter.phpt (partial: the DatePeriod getters for
/// recurrences are not ported)
#[test]
fn dateperiod_getter() {
    let z = "UTC";
    let start = DT::new_in("2000-01-01 00:00:00", "Europe/Berlin", z);
    let end = DT::new_in("2000-01-31 00:00:00", "UTC", z);
    let interval = php::date_interval_create("P1D").unwrap();
    let mut out = String::new();
    out += &vd_str(&start.format("Y-m-d H:i:s"));
    out += &vd_str(&start.timezone_name());
    out += &vd_str(&end.format("Y-m-d H:i:s"));
    out += &vd_str(&end.timezone_name());
    out += &vd_str(&interval_format(&interval, "%R%y-%m-%d-%h-%i-%s"));
    assert_text(
        &out,
        r#"string(19) "2000-01-01 00:00:00"
string(13) "Europe/Berlin"
string(19) "2000-01-31 00:00:00"
string(3) "UTC"
string(12) "+0-0-1-0-0-0"
"#,
    );
}

/// ext/date/tests/DatePeriod_serialize-002.phpt (partial: only the iteration; the
/// serialization and var_dump of the period are not ported)
#[test]
fn dateperiod_serialize_002() {
    let z = "Europe/London";
    let s = DT::new("1978-12-22 09:15:00 Europe/Amsterdam", z);
    let e = DT::new("2022-04-29 15:51:56 Europe/London", z);
    let i = php::date_interval_create("P2Y6M").unwrap();
    assert_eq!(
        s.var_dump("DateTimeImmutable", 2),
        r#"object(DateTimeImmutable)#2 (3) {
  ["date"]=>
  string(26) "1978-12-22 09:15:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(16) "Europe/Amsterdam"
}
"#
    );
    assert_eq!(
        e.var_dump("DateTimeImmutable", 3),
        r#"object(DateTimeImmutable)#3 (3) {
  ["date"]=>
  string(26) "2022-04-29 15:51:56.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
"#
    );
    assert_eq!(
        V::Obj("DateInterval".into(), 4, interval_props(&i)).var_dump(),
        r#"object(DateInterval)#4 (10) {
  ["y"]=>
  int(2)
  ["m"]=>
  int(6)
  ["d"]=>
  int(0)
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
"#
    );
    let mut out = String::new();
    for d in period_dates(&s, &i, &e, false) {
        out += &d.format(DATE_ISO8601);
        out += "\n";
    }
    assert_text(
        &out,
        "1978-12-22T09:15:00+0100
1981-06-22T09:15:00+0200
1983-12-22T09:15:00+0100
1986-06-22T09:15:00+0200
1988-12-22T09:15:00+0100
1991-06-22T09:15:00+0200
1993-12-22T09:15:00+0100
1996-06-22T09:15:00+0200
1998-12-22T09:15:00+0100
2001-06-22T09:15:00+0200
2003-12-22T09:15:00+0100
2006-06-22T09:15:00+0200
2008-12-22T09:15:00+0100
2011-06-22T09:15:00+0200
2013-12-22T09:15:00+0100
2016-06-22T09:15:00+0200
2018-12-22T09:15:00+0100
2021-06-22T09:15:00+0200
",
    );
}

/// ext/date/tests/DatePeriod_wrong_arguments.phpt (partial: only that the
/// constructor arguments parse; the TypeError is not ported)
#[test]
fn dateperiod_wrong_arguments() {
    let z = "UTC";
    assert!(DT::try_new("2023-01-13 12:29:30", None, z).is_ok());
    assert!(DT::try_new("2023-01-16 16:49:29", None, z).is_ok());
    assert!(php::date_interval_create_from_date_string("tomorrow").is_some());
    let r = php_timelib::strtointerval("R4/2012-07-01T00:00:00Z/P7D");
    assert!(r.errors.is_empty());
    assert!(r.begin.is_some());
    assert!(r.period.is_some());
    assert_eq!(r.recurrences, Some(4));
}

/// ext/date/tests/DateTimeZone_compare_basic1.phpt (partial: the uninitialised
/// object comparison is not ported)
#[test]
fn datetimezone_compare_basic1() {
    let mut out = String::new();
    let mut compare = |a: &str, b: &str| {
        let tz1 = php::timezone_open(a).unwrap();
        let tz2 = php::timezone_open(b).unwrap();
        let eq = timezone_equals(&tz1, &tz2).unwrap();
        // `<` and `>` are false for both "equal" (0) and "uncomparable" (1).
        let _ = write!(
            out,
            "compare {a} with {b}\n< {}= {}> {}",
            vd_bool(false),
            vd_bool(eq),
            vd_bool(false)
        );
    };
    for (a, b) in [
        ("+0200", "-0200"),
        ("EST", "PST"),
        ("Europe/Amsterdam", "Europe/Berlin"),
    ] {
        compare(a, a);
        compare(a, b);
    }
    let berlin = php::timezone_open("Europe/Berlin").unwrap();
    let cet = php::timezone_open("CET").unwrap();
    match timezone_equals(&berlin, &cet) {
        Ok(b) => out += &vd_bool(b),
        Err(e) => {
            let _ = writeln!(out, "DateException: {e}");
        }
    }
    assert_text(
        &out,
        "compare +0200 with +0200
< bool(false)
= bool(true)
> bool(false)
compare +0200 with -0200
< bool(false)
= bool(false)
> bool(false)
compare EST with EST
< bool(false)
= bool(true)
> bool(false)
compare EST with PST
< bool(false)
= bool(false)
> bool(false)
compare Europe/Amsterdam with Europe/Amsterdam
< bool(false)
= bool(true)
> bool(false)
compare Europe/Amsterdam with Europe/Berlin
< bool(false)
= bool(false)
> bool(false)
DateException: Cannot compare two different kinds of DateTimeZone objects
",
    );
}

/// ext/date/tests/DateTimeZone_getOffset_basic1.phpt ("now" is the fixed
/// `common::NOW`; the expectation accepts both DST and standard offsets)
#[test]
fn datetimezone_getoffset_basic1() {
    let date = DT::new("GMT", "GMT");
    let off = |z: &str| timezone_offset_get(&php::timezone_open(z).unwrap(), &date.t);
    assert!([0, 3600].contains(&off("Europe/London")));
    assert!([-18000, -14400].contains(&off("America/New_York")));
    assert!([-28800, -25200].contains(&off("America/Los_Angeles")));
}

/// ext/date/tests/DateTimeZone_serialization.phpt (partial: the zone types and
/// names `__serialize`/`__unserialize` produce, via `timezone_initialize`)
#[test]
fn datetimezone_serialization() {
    let tz = php::timezone_open("CEST").unwrap();
    let mut out = V::Arr(tz_props(&tz)).var_dump();
    for (i, name) in ["Europe/London", "CEST", "+0130"].into_iter().enumerate() {
        let tz = php::timezone_open(name).unwrap();
        out += &V::Obj("DateTimeZone".into(), i as u32 + 1, tz_props(&tz)).var_dump();
    }
    assert_text(
        &out,
        r#"array(2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(4) "CEST"
}
object(DateTimeZone)#1 (2) {
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
object(DateTimeZone)#2 (2) {
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(4) "CEST"
}
object(DateTimeZone)#3 (2) {
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:30"
}
"#,
    );
}

/// ext/date/tests/DateTime_compare_basic1.phpt
#[test]
fn datetime_compare_basic1() {
    let z = "Europe/London";
    let mk = || DT::new("2009-02-12 12:47:41 GMT", z);
    let (mut o1, mut o2, mut o3, mut o4) = (mk(), mk(), mk(), mk());
    let eq = |a: &DT, b: &DT| vd_bool(a.t.compare(&b.t) == Ordering::Equal);
    let mut out = String::from("Simple test for DateTime compare object handler\n");
    out += "\n-- All the following tests should compare equal --\n";
    for (a, b) in [
        (&o1, &o1),
        (&o1, &o2),
        (&o1, &o3),
        (&o1, &o4),
        (&o2, &o3),
        (&o2, &o4),
        (&o3, &o4),
    ] {
        out += &eq(a, b);
    }
    assert!(o1.modify("+1 day"));
    out += "\n-- The following test should still compare equal --\n";
    out += &eq(&o1, &o1);
    out += "\n-- All the following tests should now compare NOT equal --\n";
    out += &eq(&o1, &o2);
    out += &eq(&o1, &o3);
    out += &eq(&o1, &o4);
    out += "\n-- All the following tests should again compare equal --\n";
    assert!(o2.modify("+1 day"));
    assert!(o3.modify("+1 day"));
    assert!(o4.modify("+1 day"));
    out += &eq(&o1, &o2);
    out += &eq(&o1, &o3);
    out += &eq(&o1, &o4);
    assert_text(
        &out,
        "Simple test for DateTime compare object handler

-- All the following tests should compare equal --
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)
bool(true)

-- The following test should still compare equal --
bool(true)

-- All the following tests should now compare NOT equal --
bool(false)
bool(false)
bool(false)

-- All the following tests should again compare equal --
bool(true)
bool(true)
bool(true)
",
    );
}

/// ext/date/tests/DateTime_createFromImmutable-002.phpt (partial: the TypeError is
/// not ported)
#[test]
fn datetime_createfromimmutable_002() {
    let z = "Europe/London";
    let current = "2014-03-02 16:24:08";
    let i = DT::new(current, z);
    let mut m = i.clone();
    let mut out = m.var_dump("MyDateTime", 2);
    assert!(m.modify("+ 1 hour"));
    out += &vd_bool(i.format("Y-m-d H:i:s") == current);
    assert_text(
        &out,
        r#"object(MyDateTime)#2 (3) {
  ["date"]=>
  string(26) "2014-03-02 16:24:08.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}
bool(true)
"#,
    );
    assert_eq!(m.format("Y-m-d H:i:s"), "2014-03-02 17:24:08");
}

/// ext/date/tests/DateTime_fix_createFromFormat.phpt
#[test]
fn datetime_fix_createfromformat() {
    let z = "Europe/London";
    let orig = DT::new("2012-11-29 17:00:00", z);
    let from_format =
        DT::from_format("D., M# j, Y g:iA", "Thu., Nov. 29, 2012 5:00PM", None, z).unwrap();
    let out = format!(
        "*** Testing clone on DateTime objects ***\nFormat method: {}\ncreateFromFormat method: {}\n",
        orig.format("D., M. j, Y g:iA"),
        from_format.format("D., M. j, Y g:iA")
    );
    assert_text(
        &out,
        "*** Testing clone on DateTime objects ***
Format method: Thu., Nov. 29, 2012 5:00PM
createFromFormat method: Thu., Nov. 29, 2012 5:00PM
",
    );
}

/// ext/date/tests/DateTime_modify_basic1.phpt
#[test]
fn datetime_modify_basic1() {
    let z = "Europe/London";
    let mut datetime = DT::new("2009-01-31 14:28:41", z);
    let mut out = String::from("*** Testing DateTime::modify() : basic functionality ***\n");
    assert!(datetime.modify("+1 day"));
    let _ = writeln!(out, "After modification 1: {}", datetime.format("D, d M Y"));
    assert!(datetime.modify("+1 week 2 days 4 hours 2 seconds"));
    let _ = writeln!(
        out,
        "After modification 2: {}",
        datetime.format("D, d M Y H:i:s")
    );
    assert!(datetime.modify("next Thursday"));
    let _ = writeln!(out, "After modification 3: {}", datetime.format("D, d M Y"));
    assert!(datetime.modify("last Sunday"));
    let _ = writeln!(out, "After modification 4: {}", datetime.format("D, d M Y"));
    assert_text(
        &out,
        "*** Testing DateTime::modify() : basic functionality ***
After modification 1: Sun, 01 Feb 2009
After modification 2: Tue, 10 Feb 2009 18:28:43
After modification 3: Thu, 12 Feb 2009
After modification 4: Sun, 08 Feb 2009
",
    );
}

/// ext/date/tests/DateTime_setTime_basic1.phpt
#[test]
fn datetime_settime_basic1() {
    let z = "Europe/London";
    let mut datetime = DT::new("2009-01-31 15:14:10", z);
    let mut out = String::from("*** Testing DateTime::setTime() : basic functionality ***\n");
    let _ = writeln!(out, "Initial date: {}", datetime.format(DATE_RFC2822));
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
        let _ = writeln!(
            out,
            "After modification{} {}",
            n + 1,
            datetime.format(DATE_RFC2822)
        );
    }
    assert_text(
        &out,
        "*** Testing DateTime::setTime() : basic functionality ***
Initial date: Sat, 31 Jan 2009 15:14:10 +0000
After modification1 Sat, 31 Jan 2009 17:20:00 +0000
After modification2 Sat, 31 Jan 2009 19:05:59 +0000
After modification3 Sun, 01 Feb 2009 00:10:00 +0000
After modification4 Mon, 02 Feb 2009 23:35:47 +0000
After modification5 Wed, 04 Feb 2009 06:25:00 +0000
",
    );
}

/// ext/date/tests/big_year.phpt
#[test]
fn big_year() {
    let z = "America/Toronto";
    let t = mktime(0, 0, 0, 1, 1, 2922770265, z);
    let s = date("r", t, z);
    assert!(
        s.ends_with(", 01 Jan 2922770265 00:00:00 -0500"),
        "got {s:?}"
    );
}

/// ext/date/tests/bug-gh8964-002.phpt
#[test]
fn bug_gh8964_002() {
    let z = "UTC";
    let mut out = String::new();
    for seconds in 0..3 {
        let actual = DT::new("2022-07-21 15:00:10", z);
        let mut delta = php::date_interval_create(&format!("PT{seconds}S")).unwrap();
        // `$delta->f = -0.9`
        delta.diff.us = (-0.9f64 * 1_000_000.0) as i64;
        let mut lower = actual.clone();
        lower.sub_interval(&delta);
        let mut upper = actual.clone();
        upper.add_interval(&delta);
        for d in [&lower, &actual, &upper] {
            out += &d.format("H:i:s.u U");
            out += "\n";
        }
        out += &vd_bool(actual.t.compare(&lower.t) == Ordering::Less);
        out += &vd_bool(actual.t.compare(&upper.t) == Ordering::Greater);
    }
    assert_text(
        &out,
        "15:00:10.900000 1658415610
15:00:10.000000 1658415610
15:00:09.100000 1658415609
bool(true)
bool(true)
15:00:09.900000 1658415609
15:00:10.000000 1658415610
15:00:10.100000 1658415610
bool(false)
bool(false)
15:00:08.900000 1658415608
15:00:10.000000 1658415610
15:00:11.100000 1658415611
bool(false)
bool(false)
",
    );
}

/// ext/date/tests/bug20382-1.phpt
#[test]
fn bug20382_1() {
    let z = "Europe/Amsterdam";
    let t_stamp = mktime(17, 17, 17, 10, 27, 2004, z);
    let r = strtotime("Monday", t_stamp, z).unwrap();
    let out = format!(
        "tStamp={}\nresult={}\nwanted=Monday 2004-11-01 00:00:00 CET\n",
        date("l Y-m-d H:i:s T", t_stamp, z),
        date("l Y-m-d H:i:s T", r, z)
    );
    assert_text(
        &out,
        "tStamp=Wednesday 2004-10-27 17:17:17 CEST
result=Monday 2004-11-01 00:00:00 CET
wanted=Monday 2004-11-01 00:00:00 CET
",
    );
}

/// ext/date/tests/bug26317.phpt
#[test]
fn bug26317() {
    let z = "GMT0";
    let mut out = String::new();
    for s in [
        "2003-11-19 16:20:42 Z",
        "2003-11-19 09:20:42 T",
        "2003-11-19 19:20:42 C",
    ] {
        out += &date("Y-m-d H:i:s\n", strtotime_abs(s, z).unwrap(), z);
    }
    assert_text(
        &out,
        "2003-11-19 16:20:42
2003-11-19 16:20:42
2003-11-19 16:20:42
",
    );
}

/// ext/date/tests/bug28599.phpt
#[test]
fn bug28599() {
    let ts = strtotime("+30 minutes", 1100535573, "Europe/Amsterdam").unwrap();
    assert_eq!(gmdate("d-m-Y H:i:s", ts), "15-11-2004 16:49:33");
}

/// ext/date/tests/bug32086.phpt
#[test]
fn bug32086() {
    let z = "America/Sao_Paulo";
    let mut out = String::new();
    for (a, b, c) in [
        ("2004-11-01", "2004-11-02", "2004-11-03"),
        ("2005-02-19", "2005-02-20", "2005-02-21"),
    ] {
        let g = strtotime_abs(a, z).unwrap();
        let i = strtotime_abs(&format!("{a} +1 day"), z).unwrap();
        let j = strtotime("+1 day", g, z).unwrap();
        let k = strtotime_abs(b, z).unwrap();
        let l = strtotime_abs(c, z).unwrap();
        for ts in [g, i, j, k, l] {
            let _ = writeln!(out, "{ts}");
        }
        for ts in [g, i, j, k, l] {
            out += &date("Y-m-d H:i:s T\n", ts, z);
        }
    }
    assert_text(
        &out,
        "1099278000
1099364400
1099364400
1099364400
1099447200
2004-11-01 00:00:00 -03
2004-11-02 01:00:00 -02
2004-11-02 01:00:00 -02
2004-11-02 01:00:00 -02
2004-11-03 00:00:00 -02
1108778400
1108868400
1108868400
1108868400
1108954800
2005-02-19 00:00:00 -02
2005-02-20 00:00:00 -03
2005-02-20 00:00:00 -03
2005-02-20 00:00:00 -03
2005-02-21 00:00:00 -03
",
    );
}

/// ext/date/tests/bug33414-2.phpt
#[test]
fn bug33414_2() {
    let cases = [
        (
            "Pacific/Rarotonga",
            " - wrong day.",
            1,
            "next Tuesday",
            "Tuesday",
        ),
        (
            "Atlantic/South_Georgia",
            " - wrong day.",
            1,
            "next Tuesday",
            "Tuesday",
        ),
        (
            "America/Port-au-Prince",
            " - wrong day.",
            12871,
            "next Monday",
            "Monday",
        ),
        (
            "Pacific/Enderbury",
            " - wrong day, off by 2 days.",
            1,
            "next Monday",
            "Monday",
        ),
        (
            "Pacific/Kiritimati",
            " - wrong day, off by 2 days.",
            1,
            "next Monday",
            "Monday",
        ),
        (
            "America/Managua",
            " - wrong day.",
            12879,
            "next Tuesday",
            "Tuesday",
        ),
        (
            "Pacific/Pitcairn",
            " - wrong day.",
            1,
            "next Wednesday",
            "Wednesday",
        ),
        (
            "Pacific/Fakaofo",
            " - wrong day.",
            1,
            "next Saturday",
            "Saturday",
        ),
        (
            "Pacific/Johnston",
            " - wrong day.",
            1,
            "next Friday",
            "Friday",
        ),
    ];
    let mut out = String::new();
    for (z, note, day, rel, wanted) in cases {
        let t_stamp = mktime(17, 17, 17, 1, day, 1970, z);
        let r = strtotime(rel, t_stamp, z).unwrap();
        let _ = write!(
            out,
            "TZ={z}{note}\ntStamp={}\nresult={}\nwanted={wanted}            00:00:00\n\n",
            date("l Y-m-d H:i:s T I", t_stamp, z),
            date("l Y-m-d H:i:s T I", r, z)
        );
    }
    assert_text(
        &out,
        "TZ=Pacific/Rarotonga - wrong day.
tStamp=Thursday 1970-01-01 17:17:17 -1030 0
result=Tuesday 1970-01-06 00:00:00 -1030 0
wanted=Tuesday            00:00:00

TZ=Atlantic/South_Georgia - wrong day.
tStamp=Thursday 1970-01-01 17:17:17 -02 0
result=Tuesday 1970-01-06 00:00:00 -02 0
wanted=Tuesday            00:00:00

TZ=America/Port-au-Prince - wrong day.
tStamp=Monday 2005-03-28 17:17:17 EST 0
result=Monday 2005-04-04 00:00:00 EDT 1
wanted=Monday            00:00:00

TZ=Pacific/Enderbury - wrong day, off by 2 days.
tStamp=Thursday 1970-01-01 17:17:17 -12 0
result=Monday 1970-01-05 00:00:00 -12 0
wanted=Monday            00:00:00

TZ=Pacific/Kiritimati - wrong day, off by 2 days.
tStamp=Thursday 1970-01-01 17:17:17 -1040 0
result=Monday 1970-01-05 00:00:00 -1040 0
wanted=Monday            00:00:00

TZ=America/Managua - wrong day.
tStamp=Tuesday 2005-04-05 17:17:17 CST 0
result=Tuesday 2005-04-12 00:00:00 CDT 1
wanted=Tuesday            00:00:00

TZ=Pacific/Pitcairn - wrong day.
tStamp=Thursday 1970-01-01 17:17:17 -0830 0
result=Wednesday 1970-01-07 00:00:00 -0830 0
wanted=Wednesday            00:00:00

TZ=Pacific/Fakaofo - wrong day.
tStamp=Thursday 1970-01-01 17:17:17 -11 0
result=Saturday 1970-01-03 00:00:00 -11 0
wanted=Saturday            00:00:00

TZ=Pacific/Johnston - wrong day.
tStamp=Thursday 1970-01-01 17:17:17 HST 0
result=Friday 1970-01-02 00:00:00 HST 0
wanted=Friday            00:00:00
",
    );
}

/// ext/date/tests/bug33562.phpt
#[test]
fn bug33562() {
    assert_eq!(format!("[{}]\ndone", date("", NOW, "GMT")), "[]\ndone");
}

/// ext/date/tests/bug34304.phpt
#[test]
fn bug34304() {
    let z = "UTC";
    let f = "o\\-\\WW\\-N";
    assert_eq!(
        date(f, strtotime_abs("2 January 2005", z).unwrap(), z),
        "2004-W53-7"
    );
    assert_eq!(
        date(f, strtotime_abs("9 January 2005", z).unwrap(), z),
        "2005-W01-7"
    );
}

/// ext/date/tests/bug35422.phpt
#[test]
fn bug35422() {
    let z = "UTC";
    for s in ["July 1, 2000 00:00:00 UTC", "July 1, 2000 00:00:00 GMT"] {
        assert_eq!(
            date(DATE_ISO8601, strtotime_abs(s, z).unwrap(), z),
            "2000-07-01T00:00:00+0000"
        );
    }
}

/// ext/date/tests/bug35699.phpt
#[test]
fn bug35699() {
    let z = "UTC";
    let mut out = String::new();
    for s in ["1964-06-06", "1963-06-06", "1964-01-06"] {
        out += &date(DATE_ISO8601, strtotime_abs(s, z).unwrap(), z);
        out += "\n";
    }
    assert_text(
        &out,
        "1964-06-06T00:00:00+0000
1963-06-06T00:00:00+0000
1964-01-06T00:00:00+0000
",
    );
}

/// ext/date/tests/bug36599.phpt
#[test]
fn bug36599() {
    let z = "UTC";
    let ts = strtotime_abs("2006-03-03 08:47:55", z).unwrap();
    assert_eq!(date(DATE_ATOM, ts, z), "2006-03-03T08:47:55+00:00");
    assert_eq!(date(DATE_W3C, ts, z), "2006-03-03T08:47:55+00:00");
}

/// ext/date/tests/bug37747.phpt
#[test]
fn bug37747() {
    assert_eq!(strtotime_abs("nextyear", "Europe/Oslo"), None);
}

/// ext/date/tests/bug41523.phpt (partial: the 32-bit strtotime() overflow is not
/// ported)
#[test]
fn bug41523() {
    let z = "UTC";
    let mut out = date_parse("0000-00-00 00:00:00").var_dump();
    let dt = DT::new("0000-00-00 00:00:00", z);
    out += &dt.var_dump("DateTime", 1);
    out += &dt.format(DATE_ISO8601);
    out += "\n";
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
object(DateTime)#1 (3) {
  ["date"]=>
  string(27) "-0001-11-30 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
-0001-11-30T00:00:00+0000
"#,
    );
}

/// ext/date/tests/bug42910.phpt
#[test]
fn bug42910() {
    let z = "America/Los_Angeles";
    let mut foo = DT::new("2007-03-11", z);
    let mut bar = DT::new("2007-03-11T00:00:00-0800", z);
    let line = |d: &DT| {
        format!(
            "{} - {} - {}\r\n",
            d.format(DATE_ISO8601),
            d.timezone_name(),
            d.format("U")
        )
    };
    let mut out = line(&foo) + &line(&bar);
    foo.set_date(2007, 3, 12);
    bar.set_date(2007, 3, 12);
    out += &line(&foo);
    out += &line(&bar);
    let d = DT::new("2007-11-04 12:00:00+0200", "Australia/Sydney");
    out += &vd_str(&d.format("O e"));
    assert_eq!(
        out,
        "2007-03-11T00:00:00-0800 - America/Los_Angeles - 1173600000\r
2007-03-11T00:00:00-0800 - -08:00 - 1173600000\r
2007-03-12T00:00:00-0700 - America/Los_Angeles - 1173682800\r
2007-03-12T00:00:00-0800 - -08:00 - 1173686400\r
string(12) \"+0200 +02:00\"
"
    );
}

/// ext/date/tests/bug43960.phpt
#[test]
fn bug43960() {
    assert_eq!(strtotime_abs("i like to eat slices at work", "UTC"), None);
}

/// ext/date/tests/bug45543.phpt
#[test]
fn bug45543() {
    let z = "UTC";
    let mut out = String::new();
    for test_date in ["2008-01-01 12:00:00 PDT", "2008-01-01 12:00:00 +02:00"] {
        let d1 = DT::new(test_date, z);
        let mut d2 = DT::new("2008-01-01 12:00:00 UTC", z);
        out += &format!("{}\n{}\n", d1.format(DATE_ISO8601), d2.format(DATE_ISO8601));
        let tz = PhpTimeZone::from_time(&d1.t).unwrap();
        php::date_timezone_set(&mut d2.t, &tz);
        out += &format!(
            "{}\n{}\n\n",
            d1.format(DATE_ISO8601),
            d2.format(DATE_ISO8601)
        );
    }
    assert_text(
        &out,
        "2008-01-01T12:00:00-0700
2008-01-01T12:00:00+0000
2008-01-01T12:00:00-0700
2008-01-01T05:00:00-0700

2008-01-01T12:00:00+0200
2008-01-01T12:00:00+0000
2008-01-01T12:00:00+0200
2008-01-01T14:00:00+0200
",
    );
}

/// ext/date/tests/bug46268.phpt
#[test]
fn bug46268() {
    let z = "Asia/Tokyo";
    let mut now = DT::new("2008-10-10 01:02:03", z);
    let mut out = now.format("Y-m-d H:i:s") + "\n";
    assert!(now.modify("1 day"));
    out += &(now.format("Y-m-d H:i:s") + "\n");
    assert!(now.modify("1 hour"));
    out += &(now.format("Y-m-d H:i:s") + "\n");
    now.set_time(0, 0, 0, 0);
    out += &(now.format("Y-m-d H:i:s") + "\n");
    assert_text(
        &out,
        "2008-10-10 01:02:03
2008-10-11 01:02:03
2008-10-11 02:02:03
2008-10-11 00:00:00
",
    );
}

/// ext/date/tests/bug48678.phpt (partial: the serialize() round trip is not ported)
#[test]
fn bug48678() {
    let x = php::date_interval_create("P3Y6M4DT12H30M5S").unwrap();
    assert_text(
        &interval_print_r(&x),
        "DateInterval Object
(
    [y] => 3
    [m] => 6
    [d] => 4
    [h] => 12
    [i] => 30
    [s] => 5
    [f] => 0
    [invert] => 0
    [days] =>\x20
    [from_string] =>\x20
)
",
    );
}

/// ext/date/tests/bug50055-001.phpt
#[test]
fn bug50055_001() {
    let z = "UTC";
    let now = "2010-03-07 13:21:38 UTC";
    let mut out = String::new();
    let mut da1 = DT::new(now, z);
    let ds1 = DT::new(now, z);
    let i = php::date_interval_create_from_date_string("third Tuesday of next month").unwrap();
    out += &(da1.format(DATE_ISO8601) + "\n");
    da1.add_interval(&i);
    out += &(da1.format(DATE_ISO8601) + "\n");
    // date_sub() warns "Only non-special relative time specifications are
    // supported for subtraction" and leaves the object unchanged.
    assert!(php::date_sub(&ds1.t, &i).is_none());
    let mut da2 = DT::new(now, z);
    let ds2 = DT::new(now, z);
    let i2 = php::date_interval_create_from_date_string("third Tuesday of last month").unwrap();
    out += &(da2.format(DATE_ISO8601) + "\n");
    da2.add_interval(&i2);
    out += &(da2.format(DATE_ISO8601) + "\n");
    assert!(php::date_sub(&ds2.t, &i).is_none());
    assert_text(
        &out,
        "2010-03-07T13:21:38+0000
2010-04-20T13:21:38+0000
2010-03-07T13:21:38+0000
2010-02-16T13:21:38+0000
",
    );
}

/// ext/date/tests/bug51393.phpt
#[test]
fn bug51393() {
    let z = "UTC";
    let mut out = String::new();
    for (f, s) in [
        ("O", "+0800"),
        ("P", "+08:00"),
        ("O", "-0800"),
        ("P", "-08:00"),
        ("[O]", "[+0800]"),
        ("[P]", "[+08:00]"),
        ("[O]", "[-0800]"),
        ("[P]", "[-08:00]"),
        ("O", "GMT+0800"),
        ("P", "GMT+08:00"),
        ("O", "GMT-0800"),
        ("P", "GMT-08:00"),
        ("[O]", "[GMT+0800]"),
        ("[P]", "[GMT+08:00]"),
        ("[O]", "[GMT-0800]"),
        ("[P]", "[GMT-08:00]"),
    ] {
        let dt = DT::from_format(f, s, None, z).unwrap_or_else(|| panic!("{f} {s}"));
        out += &vd_int(dt.offset());
    }
    out += &vd_bool(DT::from_format("O", "invalid", None, z).is_some());
    assert_text(
        &out,
        "int(28800)
int(28800)
int(-28800)
int(-28800)
int(28800)
int(28800)
int(-28800)
int(-28800)
int(28800)
int(28800)
int(-28800)
int(-28800)
int(28800)
int(28800)
int(-28800)
int(-28800)
bool(false)
",
    );
}

/// ext/date/tests/bug52062-64bit.phpt
#[test]
fn bug52062_64bit() {
    let z = "UTC";
    let mut d = DT::new("@100000000000", z);
    let mut out = vd_str(&d.format("Y-m-d H:i:s U"));
    out += &vd_int(d.timestamp());
    d.set_timestamp(100000000000);
    out += &vd_str(&d.format("Y-m-d H:i:s U"));
    out += &vd_int(d.timestamp());
    let i = php::date_interval_create("PT100000000000S").unwrap();
    out += &vd_str(&interval_format(&i, "%s"));
    assert_text(
        &out,
        r#"string(32) "5138-11-16 09:46:40 100000000000"
int(100000000000)
string(32) "5138-11-16 09:46:40 100000000000"
int(100000000000)
string(12) "100000000000"
"#,
    );
}

/// ext/date/tests/bug52430.phpt
#[test]
fn bug52430() {
    assert_text(
        &date_parse("2010-1-1 24:59:59").var_dump(),
        r#"array(12) {
  ["year"]=>
  int(2010)
  ["month"]=>
  int(1)
  ["day"]=>
  int(1)
  ["hour"]=>
  int(24)
  ["minute"]=>
  int(59)
  ["second"]=>
  int(59)
  ["fraction"]=>
  float(0)
  ["warning_count"]=>
  int(1)
  ["warnings"]=>
  array(1) {
    [18]=>
    string(27) "The parsed time was invalid"
  }
  ["error_count"]=>
  int(0)
  ["errors"]=>
  array(0) {
  }
  ["is_localtime"]=>
  bool(false)
}
"#,
    );
}

/// ext/date/tests/bug52808.phpt
#[test]
fn bug52808() {
    let mut out = String::new();
    for iv in [
        "2008-05-11T15:30:00Z/2007-03-01T13:00:00Z",
        "2007-05-11T15:30:00Z/2008-03-01T13:00:00Z",
        "2007-05-11T15:30:00Z 2008-03-01T13:00:00Z",
        "2007-05-11T15:30:00Z/",
        "2007-05-11T15:30:00Z",
        "2007-05-11T15:30:00Z/:00Z",
    ] {
        match php::date_interval_create(iv) {
            Ok(di) => out += &interval_var_dump(&di, 1),
            Err(e) => {
                let _ = writeln!(out, "DateMalformedIntervalStringException: {e}");
            }
        }
    }
    out += "==DONE==\n";
    assert_text(
        &out,
        r#"object(DateInterval)#1 (10) {
  ["y"]=>
  int(1)
  ["m"]=>
  int(2)
  ["d"]=>
  int(10)
  ["h"]=>
  int(2)
  ["i"]=>
  int(30)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(1)
  ["days"]=>
  int(437)
  ["from_string"]=>
  bool(false)
}
object(DateInterval)#1 (10) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(9)
  ["d"]=>
  int(18)
  ["h"]=>
  int(21)
  ["i"]=>
  int(30)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  int(294)
  ["from_string"]=>
  bool(false)
}
object(DateInterval)#1 (10) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(9)
  ["d"]=>
  int(18)
  ["h"]=>
  int(21)
  ["i"]=>
  int(30)
  ["s"]=>
  int(0)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  int(294)
  ["from_string"]=>
  bool(false)
}
DateMalformedIntervalStringException: Failed to parse interval (2007-05-11T15:30:00Z/)
DateMalformedIntervalStringException: Failed to parse interval (2007-05-11T15:30:00Z)
DateMalformedIntervalStringException: Unknown or bad format (2007-05-11T15:30:00Z/:00Z)
==DONE==
"#,
    );
}

/// ext/date/tests/bug54340.phpt
#[test]
fn bug54340() {
    let z = "UTC";
    let interval = php::date_interval_create("P1D").unwrap();
    let mut dt = DT::new("first day of January 2011", z);
    let mut out = dt.var_dump("DateTime", 2);
    dt.add_interval(&interval);
    out += &dt.var_dump("DateTime", 2);
    let mut dt = DT::new("first day of January 2011", z);
    dt.sub_interval(&interval);
    out += &dt.var_dump("DateTime", 3);
    assert_text(
        &out,
        r#"object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2011-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2011-01-02 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
object(DateTime)#3 (3) {
  ["date"]=>
  string(26) "2010-12-31 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
"#,
    );
}

/// ext/date/tests/bug60236.phpt
#[test]
fn bug60236() {
    let z = "America/New_York";
    let mut t = DT::new("2010-07-06 18:38:28 EDT", z);
    let ts = t.format("U");
    let mut out = vd_str(&ts);
    t.set_timestamp(ts.parse().unwrap());
    out += &t.var_dump("DateTime", 1);
    assert_text(
        &out,
        r#"string(10) "1278455908"
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2010-07-06 18:38:28.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "EDT"
}
"#,
    );
}

/// ext/date/tests/bug62500.phpt (partial: only the exception message of the parent
/// constructor; the subclass property handling is not ported)
#[test]
fn bug62500() {
    assert_eq!(
        php::date_interval_create("blah").unwrap_err(),
        "Unknown or bad format (blah)"
    );
}

/// ext/date/tests/bug63391.phpt
#[test]
fn bug63391() {
    let mut out = String::from("Date         PHP\n----------   ---\n");
    for d in ["1599-12-30", "1599-12-31", "1600-01-01", "1600-01-02"] {
        out += &(DT::new(d, "UTC").format("Y-m-d   D") + "\n");
    }
    assert_text(
        &out,
        "Date         PHP
----------   ---
1599-12-30   Thu
1599-12-31   Fri
1600-01-01   Sat
1600-01-02   Sun
",
    );
}

/// ext/date/tests/bug64992.phpt
#[test]
fn bug64992() {
    let (firstyear, lastyear) = (2035, 2040);
    let z = "America/Los_Angeles";
    let mut dt = DT::new(&format!("{}-07-02", firstyear - 1), z);
    let di = php::date_interval_create("P6M").unwrap();
    let mut out = String::new();
    for _ in 0..(lastyear - firstyear) * 2 {
        dt.add_interval(&di);
        let gmto = dt.offset();
        let _ = writeln!(
            out,
            "Time Zone offset for {z} for {} is {gmto}",
            dt.format("Y-m-d")
        );
    }
    assert_text(
        &out,
        "Time Zone offset for America/Los_Angeles for 2035-01-02 is -28800
Time Zone offset for America/Los_Angeles for 2035-07-02 is -25200
Time Zone offset for America/Los_Angeles for 2036-01-02 is -28800
Time Zone offset for America/Los_Angeles for 2036-07-02 is -25200
Time Zone offset for America/Los_Angeles for 2037-01-02 is -28800
Time Zone offset for America/Los_Angeles for 2037-07-02 is -25200
Time Zone offset for America/Los_Angeles for 2038-01-02 is -28800
Time Zone offset for America/Los_Angeles for 2038-07-02 is -25200
Time Zone offset for America/Los_Angeles for 2039-01-02 is -28800
Time Zone offset for America/Los_Angeles for 2039-07-02 is -25200
",
    );
}

/// ext/date/tests/bug67109.phpt (the year depends on "now": the fixed
/// `common::NOW`)
#[test]
fn bug67109() {
    let z = "UTC";
    for s in [
        "last day of april",
        "Last day of april",
        "lAst Day of April",
    ] {
        let r = date("d.m.Y", strtotime_abs(s, z).unwrap(), z);
        assert_eq!(r.len(), 10, "{s}: {r}");
        assert!(r.starts_with("30.04."), "{s}: {r}");
    }
}

/// ext/date/tests/bug68078.phpt
#[test]
fn bug68078() {
    let z = "UTC";
    let date1 = DT::from_format("U.u", "1448889063.3531", None, z).unwrap();
    let date2 = DT::from_format("U.u", "1448889063.5216", None, z).unwrap();
    let date3 = DT::from_format("U.u", "1448889063.5216", None, z).unwrap();
    assert_ne!(date1.t.compare(&date2.t), Ordering::Equal);
    assert_eq!(date1.t.compare(&date2.t), Ordering::Less);
    assert_eq!(date2.t.compare(&date1.t), Ordering::Greater);
    assert_eq!(date2.t.compare(&date3.t), Ordering::Equal);
}

/// ext/date/tests/bug68669.phpt
#[test]
fn bug68669() {
    let z = "America/Los_Angeles";
    let a = DT::from_format("Y/m/d H:i:s", "1995/06/08 12:34:56", None, z).unwrap();
    let b = DT::from_format("Y/m/d H:i:s", "1995/06/08 12:34:56", None, z).unwrap();
    assert_text(
        &(a.var_dump("DateTime", 1) + &b.var_dump("DateTimeImmutable", 1)),
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1995-06-08 12:34:56.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(19) "America/Los_Angeles"
}
object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1995-06-08 12:34:56.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(19) "America/Los_Angeles"
}
"#,
    );
}

/// ext/date/tests/bug69806.phpt
#[test]
fn bug69806() {
    assert_eq!(
        date("Y-m-d H:i:s", 2377224000, "America/New_York"),
        "2045-05-01 00:00:00"
    );
}

/// ext/date/tests/bug71525.phpt
#[test]
fn bug71525() {
    let mut d = DT::new("2011-12-25 00:00:00", "UTC");
    assert!(d.modify("first day of next month"));
    d.set_date(2012, 1, 29);
    assert_text(
        &d.var_dump("DateTime", 1),
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2012-01-29 00:00:00.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}
"#,
    );
}

/// ext/date/tests/bug72719.phpt ("now" is the fixed `common::NOW`; the weekday
/// names do not depend on it)
#[test]
fn bug72719() {
    let mut out = String::new();
    for s in [
        "Monday next week 13:00",
        "Tuesday next week 14:00",
        "Wednesday next week 14:00",
        "Thursday next week 15:00",
        "Friday next week 16:00",
        "Saturday next week 17:00",
        "Sunday next week 18:00",
    ] {
        out += &(DT::new(s, "UTC").format("l") + "\n");
    }
    assert_text(
        &out,
        "Monday
Tuesday
Wednesday
Thursday
Friday
Saturday
Sunday
",
    );
}

/// ext/date/tests/bug73460-001.phpt
#[test]
fn bug73460_001() {
    let mut d = DT::new_in("2016-11-05 23:45:00", "America/New_York", "UTC");
    let mut out = String::new();
    for _ in 1..=20 {
        out += &(d.format("Y/m/d H:i e T") + "\n");
        d.add("PT15M");
    }
    assert_text(
        &out,
        "2016/11/05 23:45 America/New_York EDT
2016/11/06 00:00 America/New_York EDT
2016/11/06 00:15 America/New_York EDT
2016/11/06 00:30 America/New_York EDT
2016/11/06 00:45 America/New_York EDT
2016/11/06 01:00 America/New_York EDT
2016/11/06 01:15 America/New_York EDT
2016/11/06 01:30 America/New_York EDT
2016/11/06 01:45 America/New_York EDT
2016/11/06 01:00 America/New_York EST
2016/11/06 01:15 America/New_York EST
2016/11/06 01:30 America/New_York EST
2016/11/06 01:45 America/New_York EST
2016/11/06 02:00 America/New_York EST
2016/11/06 02:15 America/New_York EST
2016/11/06 02:30 America/New_York EST
2016/11/06 02:45 America/New_York EST
2016/11/06 03:00 America/New_York EST
2016/11/06 03:15 America/New_York EST
2016/11/06 03:30 America/New_York EST
",
    );
}

/// ext/date/tests/bug74057.phpt
#[test]
fn bug74057() {
    let z = "UTC";
    let mut out = String::new();
    for s in [
        "Sun 2017-01-01",
        "Mon 2017-01-02",
        "Tue 2017-01-03",
        "Wed 2017-01-04",
        "Thu 2017-01-05",
        "Fri 2017-01-06",
        "Sat 2017-01-07",
        "Sun 2017-01-08",
        "Mon 2017-01-09",
    ] {
        let base = strtotime_abs(s, z).unwrap();
        let ts = strtotime("saturday this week", base, z).unwrap();
        out += &(date("D Y-m-d", ts, z) + "\n");
    }
    assert_text(
        &out,
        "Sat 2016-12-31
Sat 2017-01-07
Sat 2017-01-07
Sat 2017-01-07
Sat 2017-01-07
Sat 2017-01-07
Sat 2017-01-07
Sat 2017-01-07
Sat 2017-01-14
",
    );
}

/// ext/date/tests/bug74671.phpt
#[test]
fn bug74671() {
    let dt = DT::new_in("2017-05-16T10:11:32", "CEST", "UTC");
    assert_text(
        &(dt.var_dump("DateTime", 1) + &vd_str(&dt.format("c"))),
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2017-05-16 10:11:32.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(4) "CEST"
}
string(25) "2017-05-16T10:11:32+02:00"
"#,
    );
}

/// ext/date/tests/bug75232.phpt (partial: the undefined-property warnings are not
/// ported)
#[test]
fn bug75232() {
    let d2 = DT::from_format("Ymd\\THis\\Z", "20170920T091600Z", None, "UTC").unwrap();
    assert_text(
        &V::Obj("DateTime".into(), 2, d2.props()).print_r(),
        "DateTime Object
(
    [date] => 2017-09-20 09:16:00.000000
    [timezone_type] => 3
    [timezone] => UTC
)
",
    );
}

/// ext/date/tests/bug76374.phpt
#[test]
fn bug76374() {
    let z = "Europe/Paris";
    for (to, from) in [
        ("2017-10-01", "2017-01-01"),
        ("2017-10-01 12:00:00", "2017-01-01 12:00:00"),
    ] {
        let interval = DT::new(to, z).diff(&DT::new(from, z));
        assert_eq!(interval.diff.m, 9, "{to} - {from}");
    }
}

/// ext/date/tests/bug77571.phpt
#[test]
fn bug77571() {
    let z = "Europe/London";
    let date3 = DT::from_format("Y-m-d H:i:s", "2019-04-01 00:00:00", None, z).unwrap();
    let mut date4 = date3.clone();
    assert!(date4.modify("+5 week"));
    assert_text(
        &interval_print_r(&date3.diff(&date4)),
        "DateInterval Object
(
    [y] => 0
    [m] => 1
    [d] => 5
    [h] => 0
    [i] => 0
    [s] => 0
    [f] => 0
    [invert] => 0
    [days] => 35
    [from_string] =>\x20
)
",
    );
}

/// ext/date/tests/bug80057.phpt ("now" is the fixed `common::NOW`)
#[test]
fn bug80057() {
    let z = "UTC";
    let now = DT::new("now", z);
    let parsed = DT::from_format("Y-m-d", "2020-09-04", None, z).unwrap();
    assert_eq!(now.format("H:i"), parsed.format("H:i"));
    assert_eq!(parsed.format("H:i"), date("H:i", NOW, z));
}

/// ext/date/tests/bug80974.phpt
#[test]
fn bug80974() {
    let z = "UTC";
    let toronto = DT::new("2012-01-01 00:00:00.000000 America/Toronto", z);
    let vancouver = DT::new("2012-01-01 00:00:00.000000 America/Vancouver", z);
    assert_eq!(interval_format(&vancouver.diff(&toronto), "%h"), "3");
}

/// ext/date/tests/bug81458.phpt
#[test]
fn bug81458() {
    let z = "UTC";
    let mut first = DT::new("2018-07-01 00:00:00.000000 America/Toronto", z);
    first.set_timezone("UTC");
    let second = DT::new("2018-07-02 00:00:00.000000 America/Toronto", z);
    let mut out = vd_int(first.diff(&second).diff.days);
    out += &vd_int(first.diff(&second).diff.d);
    let a = DT::new("2018-12-01 00:00", z);
    let b = DT::new("2018-12-02 00:01", z);
    out += &vd_int(a.diff(&b).diff.days);
    assert_text(&out, "int(1)\nint(1)\nint(1)\n");
}

/// ext/date/tests/date-time-modify-times.phpt
#[test]
fn date_time_modify_times() {
    let z = "Europe/London";
    let tests: [(&str, &[&str]); 2] = [
        (
            "2010-12-15 19:42:45 UTC",
            &[
                "october 23:00",
                "back of 4pm",
                "next week monday",
                "next week monday 10am",
                "tuesday noon",
                "first monday of January 2011",
                "first monday of January 2011 09:00",
            ],
        ),
        (
            "2010-12-15 19:42:45",
            &[
                "october 23:00",
                "march 28, 00:15",
                "march 28, 01:15",
                "march 28, 02:15",
            ],
        ),
    ];
    let mut out = String::new();
    for (start, data) in tests {
        for test in data {
            let mut d = DT::new(start, z);
            assert!(d.modify(test), "{test}");
            out += &(d.format(DATE_RFC2822) + "\n");
        }
    }
    out += "\n";
    assert_text(
        &out,
        "Fri, 15 Oct 2010 23:00:00 +0000
Wed, 15 Dec 2010 16:15:00 +0000
Mon, 20 Dec 2010 00:00:00 +0000
Mon, 20 Dec 2010 10:00:00 +0000
Tue, 21 Dec 2010 12:00:00 +0000
Mon, 03 Jan 2011 00:00:00 +0000
Mon, 03 Jan 2011 09:00:00 +0000
Fri, 15 Oct 2010 23:00:00 +0100
Sun, 28 Mar 2010 00:15:00 +0000
Sun, 28 Mar 2010 02:15:00 +0100
Sun, 28 Mar 2010 02:15:00 +0100
",
    );
}

/// ext/date/tests/date_create-2.phpt
#[test]
fn date_create_2() {
    let z = "GMT";
    let d = DT::new("2005-07-18 22:10:00 +0400", z);
    assert_eq!(
        d.format("D, d M Y H:i:s T"),
        "Mon, 18 Jul 2005 22:10:00 GMT+0400"
    );
    let d = DT::new("@1121710200 +0912", z);
    assert_eq!(
        d.format("D, d M Y H:i:s T"),
        "Mon, 18 Jul 2005 18:10:00 GMT+0000"
    );
}

/// ext/date/tests/date_diff1.phpt
#[test]
fn date_diff1() {
    let z = "Europe/London";
    let start = DT::new("2010-10-04 02:18:48 EDT", z);
    let end = DT::new("2010-11-06 18:38:28 EDT", z);
    let int = start.diff(&end);
    let out = start.var_dump("DateTime", 1) + &end.var_dump("DateTime", 2);
    assert_text(
        &out,
        r#"object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2010-10-04 02:18:48.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "EDT"
}
object(DateTime)#2 (3) {
  ["date"]=>
  string(26) "2010-11-06 18:38:28.000000"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(3) "EDT"
}
"#,
    );
    // "h" is `int(%d)` in the expectation.
    let expected = r#"object(DateInterval)#3 (10) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(1)
  ["d"]=>
  int(2)
  ["h"]=>
  int(H)
  ["i"]=>
  int(19)
  ["s"]=>
  int(40)
  ["f"]=>
  float(0)
  ["invert"]=>
  int(0)
  ["days"]=>
  int(33)
  ["from_string"]=>
  bool(false)
}
"#
    .replace("int(H)", &format!("int({})", int.diff.h));
    assert_text(&interval_var_dump(&int, 3), &expected);
}

/// ext/date/tests/date_interval_create_from_date_string_broken.phpt
#[test]
fn date_interval_create_from_date_string_broken() {
    assert!(php::date_interval_create_from_date_string("foobar").is_none());
    let r = php_timelib::strtotime("foobar", &php_timelib::builtin_tzinfo);
    let e = &r.errors.errors[0];
    let c = if e.character != 0 { e.character } else { b' ' };
    assert_eq!(
        format!(
            "Unknown or bad format (foobar) at position {} ({}): {}",
            e.position, c as char, e.message
        ),
        "Unknown or bad format (foobar) at position 0 (f): The timezone could not be found in the database"
    );
}

/// ext/date/tests/date_isodate_set_basic1.phpt
#[test]
fn date_isodate_set_basic1() {
    let z = "Europe/London";
    let mut datetime = DT::new("2009-01-30 17:57:32", z);
    let mut out = String::from("*** Testing date_isodate_set() : basic functionality ***\n");
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
        r#"*** Testing date_isodate_set() : basic functionality ***
Week 40 of 2009 is in "September"
Week 30 day 3 of 2009 is "Wed Jul 22"
..same day last year was "Wed Jul 23"
"#,
    );
}

/// ext/date/tests/date_parse_basic1.phpt
#[test]
fn date_parse_basic1() {
    let mut out = String::from("*** Testing date_parse() : basic functionality ***\n");
    for s in ["2009-02-27 10:00:00.5", "10:00:00.5", "2009-02-27"] {
        out += &date_parse(s).var_dump();
    }
    assert_text(
        &out,
        r#"*** Testing date_parse() : basic functionality ***
array(12) {
  ["year"]=>
  int(2009)
  ["month"]=>
  int(2)
  ["day"]=>
  int(27)
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
  int(2009)
  ["month"]=>
  int(2)
  ["day"]=>
  int(27)
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
"#,
    );
}

/// ext/date/tests/date_period_include_end.phpt (the DatePeriod iteration is
/// mirrored with `date_period_advance()`)
#[test]
fn date_period_include_end() {
    let z = "UTC";
    let start = DT::new("2010-06-07", z);
    let end = DT::new("2010-06-10", z);
    let interval = php::date_interval_create("P1D").unwrap();
    let mut out = String::new();
    for day in period_dates(&start, &interval, &end, true) {
        out += &(day.format("Y-m-d") + "\n");
    }
    assert_text(
        &out,
        "2010-06-07
2010-06-08
2010-06-09
2010-06-10
",
    );
}

/// ext/date/tests/date_sun_info_002.phpt
#[test]
fn date_sun_info_002() {
    let z = "Europe/Oslo";
    let ts = strtotime_abs("2007-04-13 08:31:15 UTC", z).unwrap();
    let si = php_timelib::sun_info(ts, 59.21, 9.61, &tz(z));
    let v = |s: SunValue| match s {
        SunValue::Time(t) => t,
        SunValue::Always(b) => i64::from(b),
    };
    let entries = [
        ("sunrise", v(si.sunrise)),
        ("sunset", v(si.sunset)),
        ("transit", si.transit),
        ("civil_twilight_begin", v(si.civil_twilight_begin)),
        ("civil_twilight_end", v(si.civil_twilight_end)),
        ("nautical_twilight_begin", v(si.nautical_twilight_begin)),
        ("nautical_twilight_end", v(si.nautical_twilight_end)),
        (
            "astronomical_twilight_begin",
            v(si.astronomical_twilight_begin),
        ),
        ("astronomical_twilight_end", v(si.astronomical_twilight_end)),
    ];
    let mut out = String::new();
    for (key, elem) in entries {
        let _ = writeln!(out, "{} {key}", date("Y-m-d H:i:s T", elem, z));
    }
    out += "Done\n";
    assert_text(
        &out,
        "2007-04-13 06:13:31 CEST sunrise
2007-04-13 20:30:51 CEST sunset
2007-04-13 13:22:11 CEST transit
2007-04-13 05:29:22 CEST civil_twilight_begin
2007-04-13 21:15:00 CEST civil_twilight_end
2007-04-13 04:31:43 CEST nautical_twilight_begin
2007-04-13 22:12:39 CEST nautical_twilight_end
2007-04-13 03:17:01 CEST astronomical_twilight_begin
2007-04-13 23:27:21 CEST astronomical_twilight_end
Done
",
    );
}

/// ext/date/tests/date_sunset_variation7.phpt
#[test]
fn date_sunset_variation7() {
    let inputs = [
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
    let mut out = String::from("*** Testing date_sunset() : usage variation ***\n");
    for (z, lat, lon, gmt) in inputs {
        let _ = writeln!(out, "\n--{z}--");
        for day in [11, 12] {
            let time = mktime(8, 8, 8, 8, day, 2008, z);
            match php_timelib::sunrise_sunset(
                time,
                SunFormat::String,
                lat,
                lon,
                90.0,
                Some(gmt),
                &tz(z),
                true,
            ) {
                Some(SunResult::String(s)) => out += &vd_str(&s),
                other => panic!("{z}: {other:?}"),
            }
        }
    }
    assert_text(
        &out,
        r#"*** Testing date_sunset() : usage variation ***

--Pacific/Samoa--
string(5) "18:13"
string(5) "18:13"

--US/Alaska--
string(5) "21:02"
string(5) "20:59"

--America/Chicago--
string(5) "19:52"
string(5) "19:51"

--America/Montevideo--
string(5) "18:08"
string(5) "18:08"

--Africa/Casablanca--
string(5) "19:18"
string(5) "19:17"

--Europe/Moscow--
string(5) "21:10"
string(5) "21:08"

--Asia/Hong_Kong--
string(5) "18:55"
string(5) "18:54"

--Australia/Brisbane--
string(5) "17:21"
string(5) "17:21"

--Pacific/Wallis--
string(5) "17:36"
string(5) "17:36"
"#,
    );
}

/// ext/date/tests/date_time_immutable.phpt
#[test]
fn date_time_immutable() {
    let z = "Europe/London";
    let current = "2012-12-27 16:24:08";
    let f = "Y-m-d H:i:s e";
    let mut out = String::new();
    let dump = |out: &mut String, a: &DT, b: &DT, c: &DT| {
        let _ = writeln!(out, "orig:    {}", a.format(f));
        let _ = writeln!(out, "copy:    {}", b.format(f));
        let _ = writeln!(out, "changed: {}", c.format(f));
    };
    // `$x = $z->op()` for a DateTimeImmutable (`$v` and `$z` stay unchanged) and for
    // a DateTime (all three variables are the same object).
    let both = |out: &mut String, title: &str, op: &dyn Fn(&mut DT), immutable_twice: bool| {
        if !title.is_empty() {
            out.push_str(title);
        }
        let v = DT::new(current, z);
        let mut x = v.clone();
        op(&mut x);
        dump(out, &v, &v, &x);
        let mut v = DT::new(current, z);
        op(&mut v);
        dump(out, &v, &v, &v);
        if immutable_twice {
            let v = DT::new(current, z);
            let mut x = v.clone();
            op(&mut x);
            dump(out, &v, &v, &x);
        }
    };
    both(
        &mut out,
        "modify():\n",
        &|d| assert!(d.modify("+2 days")),
        false,
    );
    both(&mut out, "\nadd():\n", &|d| d.add("P2DT2S"), false);
    both(&mut out, "\nsub():\n", &|d| d.sub("P2DT2S"), false);
    both(
        &mut out,
        "\nsetTimezone():\n",
        &|d| d.set_timezone("Asia/Tokyo"),
        true,
    );
    both(
        &mut out,
        "\nsetTime():\n",
        &|d| d.set_time(5, 7, 19, 0),
        false,
    );
    both(&mut out, "\nsetDate():\n", &|d| d.set_date(5, 7, 19), false);
    both(
        &mut out,
        "\nsetIsoDate():\n",
        &|d| d.set_isodate(2012, 2, 6),
        false,
    );
    both(
        &mut out,
        "\nsetTimestamp():\n",
        &|d| d.set_timestamp(2012234222),
        false,
    );
    assert_text(
        &out,
        "modify():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2012-12-29 16:24:08 Europe/London
orig:    2012-12-29 16:24:08 Europe/London
copy:    2012-12-29 16:24:08 Europe/London
changed: 2012-12-29 16:24:08 Europe/London

add():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2012-12-29 16:24:10 Europe/London
orig:    2012-12-29 16:24:10 Europe/London
copy:    2012-12-29 16:24:10 Europe/London
changed: 2012-12-29 16:24:10 Europe/London

sub():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2012-12-25 16:24:06 Europe/London
orig:    2012-12-25 16:24:06 Europe/London
copy:    2012-12-25 16:24:06 Europe/London
changed: 2012-12-25 16:24:06 Europe/London

setTimezone():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2012-12-28 01:24:08 Asia/Tokyo
orig:    2012-12-28 01:24:08 Asia/Tokyo
copy:    2012-12-28 01:24:08 Asia/Tokyo
changed: 2012-12-28 01:24:08 Asia/Tokyo
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2012-12-28 01:24:08 Asia/Tokyo

setTime():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2012-12-27 05:07:19 Europe/London
orig:    2012-12-27 05:07:19 Europe/London
copy:    2012-12-27 05:07:19 Europe/London
changed: 2012-12-27 05:07:19 Europe/London

setDate():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 0005-07-19 16:24:08 Europe/London
orig:    0005-07-19 16:24:08 Europe/London
copy:    0005-07-19 16:24:08 Europe/London
changed: 0005-07-19 16:24:08 Europe/London

setIsoDate():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2012-01-14 16:24:08 Europe/London
orig:    2012-01-14 16:24:08 Europe/London
copy:    2012-01-14 16:24:08 Europe/London
changed: 2012-01-14 16:24:08 Europe/London

setTimestamp():
orig:    2012-12-27 16:24:08 Europe/London
copy:    2012-12-27 16:24:08 Europe/London
changed: 2033-10-06 18:57:02 Europe/London
orig:    2033-10-06 18:57:02 Europe/London
copy:    2033-10-06 18:57:02 Europe/London
changed: 2033-10-06 18:57:02 Europe/London
",
    );
}

/// ext/date/tests/date_timezone_get_basic1.phpt
#[test]
fn date_timezone_get_basic1() {
    let mut out = String::from("*** Testing date_timezone_get() : basic functionality ***\n");
    let object = DT::new("2009-01-30 17:57:32", "Europe/London");
    out += &vd_str(&object.timezone_name());
    let mut object = DT::new("2009-01-30 17:57:32", "America/New_York");
    out += &vd_str(&object.timezone_name());
    object.set_timezone("America/Los_Angeles");
    out += &vd_str(&object.timezone_name());
    assert_text(
        &out,
        r#"*** Testing date_timezone_get() : basic functionality ***
string(13) "Europe/London"
string(16) "America/New_York"
string(19) "America/Los_Angeles"
"#,
    );
}

/// ext/date/tests/getSetMicroseconds.phpt (partial: the DateRangeError cases are not
/// ported; `setMicrosecond()` is `php_date_set_time_fraction()`)
#[test]
fn getsetmicroseconds() {
    let z = "UTC";
    let s = "2023-04-05T06:07:08.901234+00:00";
    let classes = [
        ("DateTime", false),
        ("DateTimeImmutable", true),
        ("MyDateTime", false),
        ("MyDateTimeImmutable", true),
    ];
    let mut objs: Vec<DT> = classes.iter().map(|_| DT::new(s, z)).collect();
    let mut out = String::new();
    for ((class, _), o) in classes.iter().zip(&objs) {
        let _ = writeln!(out, "{class}::getMicrosecond(): {}", o.t.us);
        out += &o.var_dump(class, 1);
    }
    for us in [0, 999999] {
        out += "##################################\n";
        for ((class, immutable), o) in classes.iter().zip(objs.iter_mut()) {
            let _ = write!(out, "{class}::setMicrosecond({us}): ");
            let mut n = o.clone();
            n.t.us = us;
            out += &n.var_dump(class, 1);
            if !immutable {
                *o = n;
            }
            let _ = writeln!(out, "{class}::getMicrosecond(): {}", o.t.us);
        }
    }
    let expected = r#"DateTime::getMicrosecond(): 901234
object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.901234"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::getMicrosecond(): 901234
object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.901234"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTime::getMicrosecond(): 901234
object(MyDateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.901234"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTimeImmutable::getMicrosecond(): 901234
object(MyDateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.901234"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
##################################
DateTime::setMicrosecond(0): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::getMicrosecond(): 0
DateTimeImmutable::setMicrosecond(0): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::getMicrosecond(): 901234
MyDateTime::setMicrosecond(0): object(MyDateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTime::getMicrosecond(): 0
MyDateTimeImmutable::setMicrosecond(0): object(MyDateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTimeImmutable::getMicrosecond(): 901234
##################################
DateTime::setMicrosecond(999999): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.999999"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::getMicrosecond(): 999999
DateTimeImmutable::setMicrosecond(999999): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.999999"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::getMicrosecond(): 901234
MyDateTime::setMicrosecond(999999): object(MyDateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.999999"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTime::getMicrosecond(): 999999
MyDateTimeImmutable::setMicrosecond(999999): object(MyDateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-04-05 06:07:08.999999"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTimeImmutable::getMicrosecond(): 901234
"#;
    assert_text(&out, expected);
}

/// ext/date/tests/gettimeofday_basic.phpt (partial: "minuteswest" and "dsttime",
/// computed at the fixed `common::NOW`; Asia/Calcutta has no DST)
#[test]
fn gettimeofday_basic() {
    let off = tz("Asia/Calcutta").get_time_zone_info(NOW);
    assert_eq!(-i64::from(off.offset) / 60, -330);
    assert!(!off.is_dst);
}

/// ext/date/tests/gh11455.phpt (partial: the DateTime properties; serialization and
/// the subclass property are not ported)
#[test]
fn gh11455() {
    let d = DT::new("2022-12-22T11:26:00Z", "UTC");
    assert_eq!(
        d.props(),
        vec![
            (
                K::S("date".into()),
                V::Str("2022-12-22 11:26:00.000000".into())
            ),
            (K::S("timezone_type".into()), V::Int(2)),
            (K::S("timezone".into()), V::Str("Z".into())),
        ]
    );
}

/// ext/date/tests/gh18304.phpt
#[test]
fn gh18304() {
    let mut di = php::date_interval_create("P0Y").unwrap();
    di.diff.d += 1;
    assert_text(
        &interval_var_dump(&di, 1),
        r#"object(DateInterval)#1 (10) {
  ["y"]=>
  int(0)
  ["m"]=>
  int(0)
  ["d"]=>
  int(1)
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
"#,
    );
}

/// ext/date/tests/gh8730.phpt
#[test]
fn gh8730() {
    let foo = DT::new_in("2022-06-08 09:15:00", "-04:00", "UTC");
    let bar = DT::new_in("2022-06-08 09:15:00", "US/Eastern", "UTC");
    assert_text(
        &interval_print_r(&foo.diff(&bar)),
        "DateInterval Object
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
    [from_string] =>\x20
)
",
    );
}

/// ext/date/tests/gmdate_basic.phpt (partial: `gmdate()` without a timestamp is not
/// ported)
#[test]
fn gmdate_basic() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    assert_eq!(
        vd_str(&gmdate(DATE_ISO8601, timestamp)),
        "string(24) \"2008-08-08T08:08:08+0000\"\n"
    );
}

/// ext/date/tests/gmdate_variation3.phpt ("now" is the fixed `common::NOW`)
#[test]
fn gmdate_variation3() {
    let timestamp = mktime(8, 8, 8, 8, 8, 2008, "UTC");
    let mut out = String::from("*** Testing gmdate() : usage variation ***\n");
    for (key, value) in [
        ("Day with leading zeros", "d"),
        ("Day without leading zeros", "j"),
        ("ISO representation", "N"),
        ("Numeric representation of day", "w"),
        ("Day of the year", "z"),
    ] {
        let _ = writeln!(out, "\n--{key}--");
        let now = gmdate(value, NOW);
        assert!(now.parse::<i64>().is_ok(), "{value}: {now}");
        out += &vd_str(&gmdate(value, timestamp));
    }
    assert_text(
        &out,
        r#"*** Testing gmdate() : usage variation ***

--Day with leading zeros--
string(2) "08"

--Day without leading zeros--
string(1) "8"

--ISO representation--
string(1) "5"

--Numeric representation of day--
string(1) "5"

--Day of the year--
string(3) "220"
"#,
    );
}

/// ext/date/tests/gmmktime_basic.phpt
#[test]
fn gmmktime_basic() {
    assert_eq!(gmmktime(8, 8, 8, 8, 8, 2008), 1218182888);
}

/// ext/date/tests/idate_variation5.phpt ("now" is the fixed `common::NOW`; the
/// expectation is `int(%d)` for each)
#[test]
fn idate_variation5() {
    for c in ['B', 'h', 'H', 'i', 'I', 's', 'U', 'Z'] {
        assert!(idate(c, NOW, "Asia/Calcutta").is_some(), "{c}");
    }
    assert_eq!(idate('U', NOW, "Asia/Calcutta"), Some(NOW as i32));
    assert_eq!(idate('Z', NOW, "Asia/Calcutta"), Some(19800));
    assert_eq!(idate('I', NOW, "Asia/Calcutta"), Some(0));
}

/// ext/date/tests/localtime_variation4.phpt
#[test]
fn localtime_variation4() {
    let z = "UTC";
    let mut out = String::from("*** Testing localtime() : usage variation ***\n");
    for (key, value) in [("Octal 0", 0), ("Octal 10", 0o12), ("Octal -10", -0o12)] {
        let _ = writeln!(out, "\n--{key}--");
        let mut t = Time {
            tz_info: Some(tz(z)),
            zone_type: ZoneType::Id,
            ..Time::new()
        };
        t.unixtime2local(value);
        let fields = [
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
        let indexed: Vec<(K, V)> = fields
            .iter()
            .enumerate()
            .map(|(i, (_, v))| (K::I(i as i64), V::Int(*v)))
            .collect();
        let assoc: Vec<(K, V)> = fields
            .iter()
            .map(|(k, v)| (K::S((*k).into()), V::Int(*v)))
            .collect();
        out += &V::Arr(indexed).var_dump();
        out += &V::Arr(assoc).var_dump();
    }
    assert_text(
        &out,
        r#"*** Testing localtime() : usage variation ***

--Octal 0--
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

--Octal 10--
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

--Octal -10--
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
}
"#,
    );
}

/// ext/date/tests/mktime_basic1.phpt (the expectation is `int(%i)`; arguments left
/// out come from the fixed `common::NOW`)
#[test]
fn mktime_basic1() {
    let z = tz("Europe/London");
    let (h, i, s, m, d, y) = (10, 30, 45, 7, 2, 1963);
    let mk = |a: [Option<i64>; 5]| php::php_mktime(h, a[0], a[1], a[2], a[3], a[4], NOW, Some(&z));
    assert!(mk([None, None, None, None, None]).is_some());
    assert!(mk([Some(i), None, None, None, None]).is_some());
    assert!(mk([Some(i), Some(s), None, None, None]).is_some());
    assert!(mk([Some(i), Some(s), Some(m), None, None]).is_some());
    assert!(mk([Some(i), Some(s), Some(m), Some(d), None]).is_some());
    let ts = mk([Some(i), Some(s), Some(m), Some(d), Some(y)]).unwrap();
    assert_eq!(
        date("Y-m-d H:i:s", ts, "Europe/London"),
        "1963-07-02 10:30:45"
    );
}

/// ext/date/tests/oom_date_create_immutable.phpt
#[test]
fn oom_date_create_immutable() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    assert!(DT::try_new(&s, None, "UTC").is_err());
}

/// ext/date/tests/oom_dateperiod_construct.phpt (partial: the ISO string fails to
/// parse)
#[test]
fn oom_dateperiod_construct() {
    let s = "\x01".repeat(4 * 1024 * 1024);
    let r = php_timelib::strtointerval(&s);
    assert!(!r.errors.errors.is_empty());
}

/// ext/date/tests/strtotime-relative.phpt
#[test]
fn strtotime_relative() {
    let z = "UTC";
    let base_time = 1204200000;
    let offsets = [
        "80412 seconds",
        "86399 seconds",
        "86400 seconds",
        "86401 seconds",
        "112913 seconds",
        "134 hours",
        "167 hours",
        "168 hours",
        "169 hours",
        "183 hours",
        "178 days",
        "179 days",
        "180 days",
        "183 days",
        "184 days",
        "115 months",
        "119 months",
        "120 months",
        "121 months",
        "128 months",
        "24 years",
        "25 years",
        "26 years",
    ];
    let mut out = String::new();
    for offset in offsets {
        for direction in ["+", "-"] {
            let ts = strtotime(&format!("{direction}{offset}"), base_time, z).unwrap();
            let _ = writeln!(out, "{direction}{offset}: {}", date(DATE_ISO8601, ts, z));
        }
    }
    assert_text(
        &out,
        "+80412 seconds: 2008-02-29T10:20:12+0000
-80412 seconds: 2008-02-27T13:39:48+0000
+86399 seconds: 2008-02-29T11:59:59+0000
-86399 seconds: 2008-02-27T12:00:01+0000
+86400 seconds: 2008-02-29T12:00:00+0000
-86400 seconds: 2008-02-27T12:00:00+0000
+86401 seconds: 2008-02-29T12:00:01+0000
-86401 seconds: 2008-02-27T11:59:59+0000
+112913 seconds: 2008-02-29T19:21:53+0000
-112913 seconds: 2008-02-27T04:38:07+0000
+134 hours: 2008-03-05T02:00:00+0000
-134 hours: 2008-02-22T22:00:00+0000
+167 hours: 2008-03-06T11:00:00+0000
-167 hours: 2008-02-21T13:00:00+0000
+168 hours: 2008-03-06T12:00:00+0000
-168 hours: 2008-02-21T12:00:00+0000
+169 hours: 2008-03-06T13:00:00+0000
-169 hours: 2008-02-21T11:00:00+0000
+183 hours: 2008-03-07T03:00:00+0000
-183 hours: 2008-02-20T21:00:00+0000
+178 days: 2008-08-24T12:00:00+0000
-178 days: 2007-09-03T12:00:00+0000
+179 days: 2008-08-25T12:00:00+0000
-179 days: 2007-09-02T12:00:00+0000
+180 days: 2008-08-26T12:00:00+0000
-180 days: 2007-09-01T12:00:00+0000
+183 days: 2008-08-29T12:00:00+0000
-183 days: 2007-08-29T12:00:00+0000
+184 days: 2008-08-30T12:00:00+0000
-184 days: 2007-08-28T12:00:00+0000
+115 months: 2017-09-28T12:00:00+0000
-115 months: 1998-07-28T12:00:00+0000
+119 months: 2018-01-28T12:00:00+0000
-119 months: 1998-03-28T12:00:00+0000
+120 months: 2018-02-28T12:00:00+0000
-120 months: 1998-02-28T12:00:00+0000
+121 months: 2018-03-28T12:00:00+0000
-121 months: 1998-01-28T12:00:00+0000
+128 months: 2018-10-28T12:00:00+0000
-128 months: 1997-06-28T12:00:00+0000
+24 years: 2032-02-28T12:00:00+0000
-24 years: 1984-02-28T12:00:00+0000
+25 years: 2033-02-28T12:00:00+0000
-25 years: 1983-02-28T12:00:00+0000
+26 years: 2034-02-28T12:00:00+0000
-26 years: 1982-02-28T12:00:00+0000
",
    );
}

/// ext/date/tests/strtotime_basic2.phpt
#[test]
fn strtotime_basic2() {
    assert_eq!(strtotime_abs("mayy 2 2009", "UTC"), None);
}

/// ext/date/tests/timestamp-in-dst.phpt
#[test]
fn timestamp_in_dst() {
    assert_eq!(
        DT::new("@1202996091", "UTC").format("c"),
        "2008-02-14T13:34:51+00:00"
    );
}

/// ext/date/tests/timezone_name_from_abbr_basic1.phpt
#[test]
fn timezone_name_from_abbr_basic1() {
    let f =
        |abbr: &str, off: i64, dst: i32| match php_timelib::timezone_id_from_abbr(abbr, off, dst) {
            Some(s) => vd_str(s),
            None => vd_bool(false),
        };
    let mut out = String::from("*** Testing timezone_name_from_abbr() : basic functionality ***\n");
    out += "-- Tests with special cases first - no lookup needed --\n";
    out += &f("GMT", -1, -1);
    out += &f("UTC", -1, -1);
    out += "-- Lookup with just name --\n";
    out += &f("CET", -1, -1);
    out += &f("EDT", -1, -1);
    out += "-- Lookup with name and offset--\n";
    out += &f("ADT", -10800, -1);
    out += &f("ADT", 14400, -1);
    out += "-- Tests without valid name - uses gmtOffset and isdst to find match --\n";
    out += &f("", 3600, 1);
    out += &f("FOO", -7200, 1);
    out += &f("", -14400, 1);
    out += &f("", -14400, 0);
    out += "-- Tests with invalid offsets --\n";
    out += &f("", 5400, -1);
    out += &f("", 62400, -1);
    assert_text(
        &out,
        r#"*** Testing timezone_name_from_abbr() : basic functionality ***
-- Tests with special cases first - no lookup needed --
string(3) "UTC"
string(3) "UTC"
-- Lookup with just name --
string(13) "Europe/Berlin"
string(16) "America/New_York"
-- Lookup with name and offset--
string(15) "America/Halifax"
string(15) "America/Halifax"
-- Tests without valid name - uses gmtOffset and isdst to find match --
string(13) "Europe/London"
string(17) "America/Sao_Paulo"
string(16) "America/New_York"
string(15) "America/Halifax"
-- Tests with invalid offsets --
bool(false)
bool(false)
"#,
    );
}

/// ext/date/tests/timezone_version_get.phpt
#[test]
fn timezone_version_get() {
    let v = php_timelib::builtin_db().version().to_string();
    let s = vd_str(&v);
    // EXPECTREGEX: string\([6-7]\) \"20[0-9][0-9]\.[1-9][0-9]?\"
    let (year, minor) = v.split_once('.').expect("version has a dot");
    assert!(
        year.len() == 4 && year.starts_with("20") && year.bytes().all(|b| b.is_ascii_digit()),
        "{s}"
    );
    assert!(
        (1..=2).contains(&minor.len())
            && minor.as_bytes()[0] != b'0'
            && minor.bytes().all(|b| b.is_ascii_digit()),
        "{s}"
    );
    assert!(
        s.starts_with("string(6)") || s.starts_with("string(7)"),
        "{s}"
    );
}

/// `php_date_initialize_from_ts_long()`: `DateTime::createFromTimestamp($int)`.
fn from_ts_long(sec: i64, usec: i64) -> DT {
    let mut t = Time {
        zone_type: ZoneType::Offset,
        ..Time::new()
    };
    t.unixtime2gmt(sec);
    t.update_ts(None);
    t.us = usec;
    DT {
        t,
        default_zone: "UTC".into(),
    }
}

/// `php_date_initialize_from_ts_double()`: `DateTime::createFromTimestamp($float)`
/// for finite values in range.
fn from_ts_double(ts: f64) -> DT {
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
    from_ts_long(sec, usec)
}

/// ext/date/tests/createFromTimestamp.phpt (partial: the DateRangeError cases for
/// out-of-range, NAN and INF timestamps are not ported)
#[test]
fn createfromtimestamp() {
    enum Ts {
        Int(i64),
        Float(f64),
    }
    let timestamps = [
        ("1696883232", Ts::Int(1696883232)),
        ("-1696883232", Ts::Int(-1696883232)),
        ("1696883232.013981", Ts::Float(1696883232.013981)),
        ("-1696883232.013981", Ts::Float(-1696883232.013981)),
        ("0.123456", Ts::Float(0.123456)),
        ("-0.123456", Ts::Float(-0.123456)),
        ("0", Ts::Int(0)),
        ("0.0", Ts::Float(0.0)),
        ("-0.0", Ts::Float(-0.0)),
        ("1599828571.235628", Ts::Float(1599828571.235628)),
        ("-1599828571.235628", Ts::Float(-1599828571.235628)),
    ];
    let mut out = String::new();
    for (label, ts) in &timestamps {
        let d = match ts {
            Ts::Int(i) => from_ts_long(*i, 0),
            Ts::Float(f) => from_ts_double(*f),
        };
        for class in ["DateTime", "DateTimeImmutable"] {
            let _ = write!(out, "{class}::createFromTimestamp({label}): ");
            out += &d.var_dump(class, 1);
        }
    }
    for class in ["MyDateTime", "MyDateTimeImmutable"] {
        let _ = write!(out, "{class}::createFromTimestamp(0): ");
        out += &from_ts_long(0, 0).var_dump(class, 1);
    }
    assert_text(
        &out,
        r#"DateTime::createFromTimestamp(1696883232): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-10-09 20:27:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(1696883232): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-10-09 20:27:12.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(-1696883232): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1916-03-25 03:32:48.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(-1696883232): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1916-03-25 03:32:48.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(1696883232.013981): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2023-10-09 20:27:12.013981"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(1696883232.013981): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2023-10-09 20:27:12.013981"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(-1696883232.013981): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1916-03-25 03:32:47.986019"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(-1696883232.013981): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1916-03-25 03:32:47.986019"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(0.123456): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.123456"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(0.123456): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.123456"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(-0.123456): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1969-12-31 23:59:59.876544"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(-0.123456): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1969-12-31 23:59:59.876544"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(0): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(0): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(0.0): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(0.0): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(-0.0): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(-0.0): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(1599828571.235628): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "2020-09-11 12:49:31.235628"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(1599828571.235628): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2020-09-11 12:49:31.235628"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTime::createFromTimestamp(-1599828571.235628): object(DateTime)#1 (3) {
  ["date"]=>
  string(26) "1919-04-22 11:10:28.764372"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
DateTimeImmutable::createFromTimestamp(-1599828571.235628): object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1919-04-22 11:10:28.764372"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTime::createFromTimestamp(0): object(MyDateTime)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
MyDateTimeImmutable::createFromTimestamp(0): object(MyDateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "1970-01-01 00:00:00.000000"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+00:00"
}
"#,
    );
}

/// `php_date_initialize_from_hash()`: `DateTime::__unserialize()` / `__set_state()`.
fn date_from_hash(
    date: &str,
    timezone_type: i32,
    timezone: &str,
    default_zone: &str,
) -> Option<DT> {
    match timezone_type {
        1 | 2 => DT::try_new(&format!("{date} {timezone}"), None, default_zone).ok(),
        3 => {
            let z = PhpTimeZone::Id(php_timelib::builtin_tzinfo(timezone)?);
            DT::try_new(date, Some(&z), default_zone).ok()
        }
        _ => None,
    }
}

/// ext/date/tests/DateTimeImmutable_serialization.phpt (partial: the serialize()
/// string is not ported; `__unserialize()` is `php_date_initialize_from_hash()`)
#[test]
fn datetimeimmutable_serialization() {
    let z = "Europe/London";
    let d = DT::new("2022-04-14 11:27:42", z);
    let mut out = String::from("Original object:\n");
    out += &d.var_dump("DateTimeImmutable", 1);
    out += "\n\nUnserialised object:\n";
    let e = date_from_hash("2022-04-14 11:27:42.000000", 3, "Europe/London", z).unwrap();
    out += &e.var_dump("DateTimeImmutable", 1);
    out += "\n\nCalling __serialize manually:\n";
    out += &V::Arr(d.props()).var_dump();
    out += "\n\nCalling __unserialize manually:\n";
    let d = date_from_hash("2022-04-14 11:27:42.541106", 3, "UTC", z).unwrap();
    out += &d.var_dump("DateTimeImmutable", 1);
    out += "\n\nCalling __unserialize a few more times, with abbreviations:\n";
    let d = date_from_hash("2022-04-14 11:27:42.541106", 2, "CEST", z).unwrap();
    out += &d.var_dump("DateTimeImmutable", 1);
    let d = date_from_hash("2022-04-14 11:27:42.541106", 1, "+0130", z).unwrap();
    out += &d.var_dump("DateTimeImmutable", 1);
    assert_text(
        &out,
        r#"Original object:
object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2022-04-14 11:27:42.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}


Unserialised object:
object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2022-04-14 11:27:42.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}


Calling __serialize manually:
array(3) {
  ["date"]=>
  string(26) "2022-04-14 11:27:42.000000"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(13) "Europe/London"
}


Calling __unserialize manually:
object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2022-04-14 11:27:42.541106"
  ["timezone_type"]=>
  int(3)
  ["timezone"]=>
  string(3) "UTC"
}


Calling __unserialize a few more times, with abbreviations:
object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2022-04-14 11:27:42.541106"
  ["timezone_type"]=>
  int(2)
  ["timezone"]=>
  string(4) "CEST"
}
object(DateTimeImmutable)#1 (3) {
  ["date"]=>
  string(26) "2022-04-14 11:27:42.541106"
  ["timezone_type"]=>
  int(1)
  ["timezone"]=>
  string(6) "+01:30"
}
"#,
    );
}

// ---------------------------------------------------------------------------------
// The DateTime diff/days/add/sub DST tests (examine_diff.inc + DateTime_data-*.inc).

/// Which segment `examine_diff()` shows (`PHPT_DATETIME_SHOW`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Diff,
    Days,
    Add,
    Sub,
}

/// One `examine_diff($end, $start, $expect_spec, ...)` call: the dates are given as
/// strings; `*_tz` means `->setTimezone(new DateTimeZone('America/New_York'))`.
struct DiffCase {
    name: &'static str,
    end: &'static str,
    end_tz: bool,
    start: &'static str,
    start_tz: bool,
    spec: &'static str,
}

/// `examine_diff()` from examine_diff.inc, with `date_default_timezone_set('America/New_York')`.
fn run_examine_diff(cases: &[DiffCase], show: Show) -> String {
    const ZONE: &str = "America/New_York";
    let mut out = String::new();
    for c in cases {
        let _ = write!(out, "{}: ", c.name);
        let mut end = DT::new(c.end, ZONE);
        if c.end_tz {
            end.set_timezone(ZONE);
        }
        let mut start = DT::new(c.start, ZONE);
        if c.start_tz {
            start.set_timezone(ZONE);
        }
        let start_date = start.format("Y-m-d H:i:s T");
        let end_date = end.format("Y-m-d H:i:s T");
        let mut expect_interval = php::date_interval_create(&format!("P{}", &c.spec[2..])).unwrap();
        if &c.spec[1..2] == "-" {
            expect_interval.diff.invert = true;
        }
        match show {
            Show::Diff => {
                let r = start.diff(&end);
                let _ = writeln!(
                    out,
                    "DIFF: {end_date} - {start_date} = **{}**",
                    interval_format(&r, "P%R%yY%mM%dDT%hH%iM%sS")
                );
            }
            Show::Days => {
                let r = start.diff(&end);
                let _ = writeln!(out, "DAYS: **{}**", interval_format(&r, "%a"));
            }
            Show::Add => {
                start.add_interval(&expect_interval);
                let _ = writeln!(
                    out,
                    "ADD: {start_date} + {} = **{}**",
                    c.spec,
                    start.format("Y-m-d H:i:s T")
                );
            }
            Show::Sub => {
                end.sub_interval(&expect_interval);
                let _ = writeln!(
                    out,
                    "SUB: {end_date} - {} = **{}**",
                    c.spec,
                    end.format("Y-m-d H:i:s T")
                );
            }
        }
    }
    out
}

/// One step of the rfc-datetime_and_daylight_saving_time tests.
enum Step {
    /// `label`, `$end`, `$start`, interval format: `$start->diff($end)`.
    Diff(&'static str, &'static str, &'static str, &'static str),
    /// `label`, `$start`, interval spec: `$start->add($interval)`.
    Add(&'static str, &'static str, &'static str),
    /// `label`, `$end`, interval spec: `$end->sub($interval)`.
    Sub(&'static str, &'static str, &'static str),
    /// `echo "\n"`.
    Blank,
}

/// Runs the rfc-datetime_and_daylight_saving_time steps with
/// `date_default_timezone_set('America/New_York')`.
fn run_steps(steps: &[Step], date_format: &str) -> String {
    const ZONE: &str = "America/New_York";
    let mut out = String::new();
    for step in steps {
        match *step {
            Step::Diff(label, end, start, fmt) => {
                let end = DT::new(end, ZONE);
                let start = DT::new(start, ZONE);
                let _ = writeln!(
                    out,
                    "{label} {} - {} = {}",
                    end.format(date_format),
                    start.format(date_format),
                    interval_format(&start.diff(&end), fmt)
                );
            }
            Step::Add(label, start, spec) => {
                let mut start = DT::new(start, ZONE);
                let before = start.format(date_format);
                start.add(spec);
                let _ = writeln!(
                    out,
                    "{label} {before} + {spec} = {}",
                    start.format(date_format)
                );
            }
            Step::Sub(label, end, spec) => {
                let mut end = DT::new(end, ZONE);
                let before = end.format(date_format);
                end.sub(spec);
                let _ = writeln!(
                    out,
                    "{label} {before} - {spec} = {}",
                    end.format(date_format)
                );
            }
            Step::Blank => out.push('\n'),
        }
    }
    out
}

/// Cases of `DateTime_data-fall-type3-type3.inc`.
const FALL_TYPE3_TYPE3: &[DiffCase] = &[
    DiffCase {
        name: "test_time_fall_type3_prev_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-10-04 02:18:48",
        start_tz: false,
        spec: "P+0Y1M2DT16H19M40S",
    },
    DiffCase {
        name: "test_time_fall_type3_prev_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_fall_type3_prev_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT6H34M5S",
    },
    DiffCase {
        name: "test_time_fall_type3_prev_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT7H36M16S",
    },
    DiffCase {
        name: "test_time_fall_type3_prev_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT9H38M27S",
    },
    DiffCase {
        name: "test_time_fall_type3_prev_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-06 18:38:28",
        start_tz: false,
        spec: "P+0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_fall_type3_dt_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P-0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_fall_type3_dt_type3_dt",
        end: "2010-11-07 00:15:35",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT0H5M15S",
    },
    DiffCase {
        name: "test_time_fall_type3_dt_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M13S",
    },
    DiffCase {
        name: "test_time_fall_type3_dt_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT2H4M24S",
    },
    DiffCase {
        name: "test_time_fall_type3_dt_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT4H6M35S",
    },
    DiffCase {
        name: "test_time_fall_type3_dt_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 00:10:20",
        start_tz: false,
        spec: "P+0Y0M1DT20H49M39S",
    },
    DiffCase {
        name: "test_time_fall_type3_redodt_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P-0Y0M0DT6H34M5S",
    },
    DiffCase {
        name: "test_time_fall_type3_redodt_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P-0Y0M0DT1H2M13S",
    },
    DiffCase {
        name: "test_time_fall_type3_redodt_type3_redodt",
        end: "2010-11-07 01:15:35 EDT",
        end_tz: true,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT0H3M2S",
    },
    DiffCase {
        name: "test_time_fall_type3_redodt_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT1H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type3_redodt_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT3H4M22S",
    },
    DiffCase {
        name: "test_time_fall_type3_redodt_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: true,
        spec: "P+0Y0M1DT19H47M26S",
    },
    DiffCase {
        name: "test_time_fall_type3_redost_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P-0Y0M0DT7H36M16S",
    },
    DiffCase {
        name: "test_time_fall_type3_redost_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P-0Y0M0DT2H4M24S",
    },
    DiffCase {
        name: "test_time_fall_type3_redost_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P-0Y0M0DT1H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type3_redost_type3_redost",
        end: "2010-11-07 01:16:54 EST",
        end_tz: true,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P+0Y0M0DT0H2M10S",
    },
    DiffCase {
        name: "test_time_fall_type3_redost_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P+0Y0M0DT2H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type3_redost_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: true,
        spec: "P+0Y0M1DT18H45M15S",
    },
    DiffCase {
        name: "test_time_fall_type3_st_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT9H38M27S",
    },
    DiffCase {
        name: "test_time_fall_type3_st_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT4H6M35S",
    },
    DiffCase {
        name: "test_time_fall_type3_st_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT3H4M22S",
    },
    DiffCase {
        name: "test_time_fall_type3_st_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT2H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type3_st_type3_st",
        end: "2010-11-07 05:19:56",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P+0Y0M0DT2H3M1S",
    },
    DiffCase {
        name: "test_time_fall_type3_st_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 03:16:55",
        start_tz: false,
        spec: "P+0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_fall_type3_post_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_fall_type3_post_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT20H49M39S",
    },
    DiffCase {
        name: "test_time_fall_type3_post_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT19H47M26S",
    },
    DiffCase {
        name: "test_time_fall_type3_post_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT18H45M15S",
    },
    DiffCase {
        name: "test_time_fall_type3_post_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-08 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_fall_type3_post_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-08 18:57:55",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M4S",
    },
    DiffCase {
        name: "test_time_fall_type3_dtsec_type3_stsec",
        end: "2010-11-07 01:00:00 EST",
        end_tz: true,
        start: "2010-11-07 01:59:59 EDT",
        start_tz: true,
        spec: "P+0Y0M0DT0H0M1S",
    },
    DiffCase {
        name: "test_time_fall_type3_stsec_type3_dtsec",
        end: "2010-11-07 01:59:59 EDT",
        end_tz: true,
        start: "2010-11-07 01:00:00 EST",
        start_tz: true,
        spec: "P-0Y0M0DT0H0M1S",
    },
];
/// Cases of `DateTime_data-spring-type3-type3.inc`.
const SPRING_TYPE3_TYPE3: &[DiffCase] = &[
    DiffCase {
        name: "test_time_spring_type3_prev_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-02-11 02:18:48",
        start_tz: false,
        spec: "P+0Y1M2DT16H19M40S",
    },
    DiffCase {
        name: "test_time_spring_type3_prev_type3_st",
        end: "2010-03-14 00:10:20",
        end_tz: false,
        start: "2010-03-13 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_spring_type3_prev_type3_dt",
        end: "2010-03-14 03:16:55",
        end_tz: false,
        start: "2010-03-13 18:38:28",
        start_tz: false,
        spec: "P+0Y0M0DT7H38M27S",
    },
    DiffCase {
        name: "test_time_spring_type3_prev_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-13 18:38:28",
        start_tz: false,
        spec: "P+0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_spring_type3_st_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P-0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_spring_type3_st_type3_st",
        end: "2010-03-14 00:15:35",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT0H5M15S",
    },
    DiffCase {
        name: "test_time_spring_type3_st_type3_dt",
        end: "2010-03-14 03:16:55",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P+0Y0M0DT2H6M35S",
    },
    DiffCase {
        name: "test_time_spring_type3_st_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-14 00:10:20",
        start_tz: false,
        spec: "P+0Y0M1DT18H49M39S",
    },
    DiffCase {
        name: "test_time_spring_type3_dt_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT7H38M27S",
    },
    DiffCase {
        name: "test_time_spring_type3_dt_type3_st",
        end: "2010-03-14 00:10:20",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P-0Y0M0DT2H6M35S",
    },
    DiffCase {
        name: "test_time_spring_type3_dt_type3_dt",
        end: "2010-03-14 05:19:56",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P+0Y0M0DT2H3M1S",
    },
    DiffCase {
        name: "test_time_spring_type3_dt_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-14 03:16:55",
        start_tz: false,
        spec: "P+0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_spring_type3_post_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-03-15 19:59:59",
        start_tz: false,
        spec: "P-0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_spring_type3_post_type3_st",
        end: "2010-03-14 00:10:20",
        end_tz: false,
        start: "2010-03-15 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT18H49M39S",
    },
    DiffCase {
        name: "test_time_spring_type3_post_type3_dt",
        end: "2010-03-14 03:16:55",
        end_tz: false,
        start: "2010-03-15 19:59:59",
        start_tz: false,
        spec: "P-0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_spring_type3_post_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-15 18:57:55",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M4S",
    },
    DiffCase {
        name: "test_time_spring_type3_stsec_type3_dtsec",
        end: "2010-03-14 03:00:00",
        end_tz: false,
        start: "2010-03-14 01:59:59",
        start_tz: false,
        spec: "P+0Y0M0DT0H0M1S",
    },
    DiffCase {
        name: "test_time_spring_type3_dtsec_type3_stsec",
        end: "2010-03-14 01:59:59",
        end_tz: false,
        start: "2010-03-14 03:00:00",
        start_tz: false,
        spec: "P-0Y0M0DT0H0M1S",
    },
];
/// Cases of `DateTime_data-fall-type2-type3.inc`.
const FALL_TYPE2_TYPE3: &[DiffCase] = &[
    DiffCase {
        name: "test_time_fall_type2_prev_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-10-04 02:18:48 EDT",
        start_tz: false,
        spec: "P+0Y1M2DT16H19M40S",
    },
    DiffCase {
        name: "test_time_fall_type2_prev_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-06 18:38:28 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_fall_type2_prev_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-06 18:38:28 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT6H34M5S",
    },
    DiffCase {
        name: "test_time_fall_type2_prev_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-06 18:38:28 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT7H36M16S",
    },
    DiffCase {
        name: "test_time_fall_type2_prev_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-06 18:38:28 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT9H38M27S",
    },
    DiffCase {
        name: "test_time_fall_type2_prev_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-06 18:38:28 EDT",
        start_tz: false,
        spec: "P+0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_fall_type2_dt_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 00:10:20 EDT",
        start_tz: false,
        spec: "P-0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_fall_type2_dt_type3_dt",
        end: "2010-11-07 00:15:35",
        end_tz: false,
        start: "2010-11-07 00:10:20 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT0H5M15S",
    },
    DiffCase {
        name: "test_time_fall_type2_dt_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-07 00:10:20 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M13S",
    },
    DiffCase {
        name: "test_time_fall_type2_dt_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-07 00:10:20 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT2H4M24S",
    },
    DiffCase {
        name: "test_time_fall_type2_dt_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-07 00:10:20 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT4H6M35S",
    },
    DiffCase {
        name: "test_time_fall_type2_dt_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 00:10:20 EDT",
        start_tz: false,
        spec: "P+0Y0M1DT20H49M39S",
    },
    DiffCase {
        name: "test_time_fall_type2_redodt_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: false,
        spec: "P-0Y0M0DT6H34M5S",
    },
    DiffCase {
        name: "test_time_fall_type2_redodt_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: false,
        spec: "P-0Y0M0DT1H2M13S",
    },
    DiffCase {
        name: "test_time_fall_type2_redodt_type3_redodt",
        end: "2010-11-07 01:15:35 EDT",
        end_tz: true,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT0H3M2S",
    },
    DiffCase {
        name: "test_time_fall_type2_redodt_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type2_redodt_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT3H4M22S",
    },
    DiffCase {
        name: "test_time_fall_type2_redodt_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 01:12:33 EDT",
        start_tz: false,
        spec: "P+0Y0M1DT19H47M26S",
    },
    DiffCase {
        name: "test_time_fall_type2_redost_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: false,
        spec: "P-0Y0M0DT7H36M16S",
    },
    DiffCase {
        name: "test_time_fall_type2_redost_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: false,
        spec: "P-0Y0M0DT2H4M24S",
    },
    DiffCase {
        name: "test_time_fall_type2_redost_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-07 01:14:44 EST",
        start_tz: false,
        spec: "P-0Y0M0DT1H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type2_redost_type3_redost",
        end: "2010-11-07 01:16:54 EST",
        end_tz: true,
        start: "2010-11-07 01:14:44 EST",
        start_tz: false,
        spec: "P+0Y0M0DT0H2M10S",
    },
    DiffCase {
        name: "test_time_fall_type2_redost_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: false,
        spec: "P+0Y0M0DT2H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type2_redost_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 01:14:44 EST",
        start_tz: false,
        spec: "P+0Y0M1DT18H45M15S",
    },
    DiffCase {
        name: "test_time_fall_type2_st_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-07 03:16:55 EST",
        start_tz: false,
        spec: "P-0Y0M0DT9H38M27S",
    },
    DiffCase {
        name: "test_time_fall_type2_st_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-07 03:16:55 EST",
        start_tz: false,
        spec: "P-0Y0M0DT4H6M35S",
    },
    DiffCase {
        name: "test_time_fall_type2_st_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-07 03:16:55 EST",
        start_tz: false,
        spec: "P-0Y0M0DT3H4M22S",
    },
    DiffCase {
        name: "test_time_fall_type2_st_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-07 03:16:55 EST",
        start_tz: false,
        spec: "P-0Y0M0DT2H2M11S",
    },
    DiffCase {
        name: "test_time_fall_type2_st_type3_st",
        end: "2010-11-07 05:19:56",
        end_tz: false,
        start: "2010-11-07 03:16:55 EST",
        start_tz: false,
        spec: "P+0Y0M0DT2H3M1S",
    },
    DiffCase {
        name: "test_time_fall_type2_st_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-07 03:16:55 EST",
        start_tz: false,
        spec: "P+0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_fall_type2_post_type3_prev",
        end: "2010-11-06 18:38:28",
        end_tz: false,
        start: "2010-11-08 19:59:59 EST",
        start_tz: false,
        spec: "P-0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_fall_type2_post_type3_dt",
        end: "2010-11-07 00:10:20",
        end_tz: false,
        start: "2010-11-08 19:59:59 EST",
        start_tz: false,
        spec: "P-0Y0M1DT20H49M39S",
    },
    DiffCase {
        name: "test_time_fall_type2_post_type3_redodt",
        end: "2010-11-07 01:12:33 EDT",
        end_tz: true,
        start: "2010-11-08 19:59:59 EST",
        start_tz: false,
        spec: "P-0Y0M1DT19H47M26S",
    },
    DiffCase {
        name: "test_time_fall_type2_post_type3_redost",
        end: "2010-11-07 01:14:44 EST",
        end_tz: true,
        start: "2010-11-08 19:59:59 EST",
        start_tz: false,
        spec: "P-0Y0M1DT18H45M15S",
    },
    DiffCase {
        name: "test_time_fall_type2_post_type3_st",
        end: "2010-11-07 03:16:55",
        end_tz: false,
        start: "2010-11-08 19:59:59 EST",
        start_tz: false,
        spec: "P-0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_fall_type2_post_type3_post",
        end: "2010-11-08 19:59:59",
        end_tz: false,
        start: "2010-11-08 18:57:55 EST",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M4S",
    },
    DiffCase {
        name: "test_time_fall_type2_dtsec_type3_stsec",
        end: "2010-11-07 01:00:00 EST",
        end_tz: true,
        start: "2010-11-07 01:59:59 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT0H0M1S",
    },
    DiffCase {
        name: "test_time_fall_type2_stsec_type3_dtsec",
        end: "2010-11-07 01:59:59 EDT",
        end_tz: true,
        start: "2010-11-07 01:00:00 EST",
        start_tz: false,
        spec: "P-0Y0M0DT0H0M1S",
    },
];
/// Cases of `DateTime_data-spring-type2-type3.inc`.
const SPRING_TYPE2_TYPE3: &[DiffCase] = &[
    DiffCase {
        name: "test_time_spring_type2_prev_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-02-11 02:18:48 EST",
        start_tz: false,
        spec: "P+0Y1M2DT16H19M40S",
    },
    DiffCase {
        name: "test_time_spring_type2_prev_type3_st",
        end: "2010-03-14 00:10:20",
        end_tz: false,
        start: "2010-03-13 18:38:28 EST",
        start_tz: false,
        spec: "P+0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_spring_type2_prev_type3_dt",
        end: "2010-03-14 03:16:55",
        end_tz: false,
        start: "2010-03-13 18:38:28 EST",
        start_tz: false,
        spec: "P+0Y0M0DT7H38M27S",
    },
    DiffCase {
        name: "test_time_spring_type2_prev_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-13 18:38:28 EST",
        start_tz: false,
        spec: "P+0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_spring_type2_st_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-03-14 00:10:20 EST",
        start_tz: false,
        spec: "P-0Y0M0DT5H31M52S",
    },
    DiffCase {
        name: "test_time_spring_type2_st_type3_st",
        end: "2010-03-14 00:15:35",
        end_tz: false,
        start: "2010-03-14 00:10:20 EST",
        start_tz: false,
        spec: "P+0Y0M0DT0H5M15S",
    },
    DiffCase {
        name: "test_time_spring_type2_st_type3_dt",
        end: "2010-03-14 03:16:55",
        end_tz: false,
        start: "2010-03-14 00:10:20 EST",
        start_tz: false,
        spec: "P+0Y0M0DT2H6M35S",
    },
    DiffCase {
        name: "test_time_spring_type2_st_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-14 00:10:20 EST",
        start_tz: false,
        spec: "P+0Y0M1DT18H49M39S",
    },
    DiffCase {
        name: "test_time_spring_type2_dt_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-03-14 03:16:55 EDT",
        start_tz: false,
        spec: "P-0Y0M0DT7H38M27S",
    },
    DiffCase {
        name: "test_time_spring_type2_dt_type3_st",
        end: "2010-03-14 00:10:20",
        end_tz: false,
        start: "2010-03-14 03:16:55 EDT",
        start_tz: false,
        spec: "P-0Y0M0DT2H6M35S",
    },
    DiffCase {
        name: "test_time_spring_type2_dt_type3_dt",
        end: "2010-03-14 05:19:56",
        end_tz: false,
        start: "2010-03-14 03:16:55 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT2H3M1S",
    },
    DiffCase {
        name: "test_time_spring_type2_dt_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-14 03:16:55 EDT",
        start_tz: false,
        spec: "P+0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_spring_type2_post_type3_prev",
        end: "2010-03-13 18:38:28",
        end_tz: false,
        start: "2010-03-15 19:59:59 EDT",
        start_tz: false,
        spec: "P-0Y0M2DT1H21M31S",
    },
    DiffCase {
        name: "test_time_spring_type2_post_type3_st",
        end: "2010-03-14 00:10:20",
        end_tz: false,
        start: "2010-03-15 19:59:59 EDT",
        start_tz: false,
        spec: "P-0Y0M1DT18H49M39S",
    },
    DiffCase {
        name: "test_time_spring_type2_post_type3_dt",
        end: "2010-03-14 03:16:55",
        end_tz: false,
        start: "2010-03-15 19:59:59 EDT",
        start_tz: false,
        spec: "P-0Y0M1DT16H43M4S",
    },
    DiffCase {
        name: "test_time_spring_type2_post_type3_post",
        end: "2010-03-15 19:59:59",
        end_tz: false,
        start: "2010-03-15 18:57:55 EDT",
        start_tz: false,
        spec: "P+0Y0M0DT1H2M4S",
    },
    DiffCase {
        name: "test_time_spring_type2_stsec_type3_dtsec",
        end: "2010-03-14 03:00:00",
        end_tz: false,
        start: "2010-03-14 01:59:59 EST",
        start_tz: false,
        spec: "P+0Y0M0DT0H0M1S",
    },
    DiffCase {
        name: "test_time_spring_type2_dtsec_type3_stsec",
        end: "2010-03-14 01:59:59",
        end_tz: false,
        start: "2010-03-14 03:00:00 EDT",
        start_tz: false,
        spec: "P-0Y0M0DT0H0M1S",
    },
];

/// Steps of `rfc-datetime_and_daylight_saving_time-type1.phpt`.
const RFC_TYPE1: &[Step] = &[
    Step::Diff(
        "fd1",
        "2010-03-14 03:00:00 -0400",
        "2010-03-14 01:59:59 -0500",
        "PT%hH%iM%sS",
    ),
    Step::Diff(
        "fd2",
        "2010-03-14 04:30:00 -0400",
        "2010-03-13 04:30:00 -0500",
        "P%dDT%hH",
    ),
    Step::Diff(
        "fd3",
        "2010-03-14 03:30:00 -0400",
        "2010-03-13 04:30:00 -0500",
        "P%dDT%hH",
    ),
    Step::Diff(
        "fd4",
        "2010-03-14 01:30:00 -0500",
        "2010-03-13 04:30:00 -0500",
        "P%dDT%hH",
    ),
    Step::Diff(
        "fd5",
        "2010-03-14 01:30:00 -0500",
        "2010-03-13 01:30:00 -0500",
        "P%dDT%hH",
    ),
    Step::Diff(
        "fd6",
        "2010-03-14 03:30:00 -0400",
        "2010-03-13 03:30:00 -0500",
        "P%dDT%hH",
    ),
    Step::Diff(
        "fd7",
        "2010-03-14 03:30:00 -0400",
        "2010-03-13 02:30:00 -0500",
        "P%dDT%hH",
    ),
    Step::Blank,
    Step::Add("fa1", "2010-03-14 01:59:59 -0500", "PT1S"),
    Step::Add("fa2", "2010-03-13 04:30:00 -0500", "P1D"),
    Step::Add("fa3", "2010-03-13 04:30:00 -0500", "PT22H"),
    Step::Add("fa4", "2010-03-13 04:30:00 -0500", "PT21H"),
    Step::Add("fa5", "2010-03-13 01:30:00 -0500", "P1D"),
    Step::Add("fa6", "2010-03-13 02:30:00 -0500", "P1D"),
    Step::Blank,
    Step::Sub("fs1", "2010-03-14 03:00:00 -0400", "PT1S"),
    Step::Sub("fs2", "2010-03-14 04:30:00 -0400", "P1D"),
    Step::Sub("fs3", "2010-03-14 03:30:00 -0400", "PT22H"),
    Step::Sub("fs4", "2010-03-14 01:30:00 -0500", "PT21H"),
    Step::Sub("fs5", "2010-03-14 01:30:00 -0500", "P1D"),
    Step::Sub("fs6", "2010-03-15 03:30:00 -0400", "P1D"),
    Step::Sub("fs7", "2010-03-15 02:30:00 -0400", "P1D"),
    Step::Blank,
    Step::Diff(
        "bd1",
        "2010-11-07 01:00:00 -0500",
        "2010-11-07 01:59:59 -0400",
        "PT%hH%iM%sS",
    ),
    Step::Diff(
        "bd2",
        "2010-11-07 04:30:00 -0500",
        "2010-11-06 04:30:00 -0400",
        "P%dDT%hH",
    ),
    Step::Diff(
        "bd3",
        "2010-11-07 03:30:00 -0500",
        "2010-11-06 04:30:00 -0400",
        "P%dDT%hH",
    ),
    Step::Diff(
        "bd4",
        "2010-11-07 02:30:00 -0500",
        "2010-11-06 04:30:00 -0400",
        "P%dDT%hH",
    ),
    Step::Diff(
        "bd5",
        "2010-11-07 01:30:00 -0500",
        "2010-11-06 04:30:00 -0400",
        "P%dDT%hH",
    ),
    Step::Diff(
        "bd6",
        "2010-11-07 01:30:00 -0400",
        "2010-11-06 04:30:00 -0400",
        "P%dDT%hH",
    ),
    Step::Diff(
        "bd7",
        "2010-11-07 01:30:00 -0400",
        "2010-11-06 01:30:00 -0400",
        "P%dDT%hH",
    ),
    Step::Diff(
        "bd8",
        "2010-11-07 01:30:00 -0500",
        "2010-11-06 01:30:00 -0400",
        "P%dDT%hH",
    ),
    Step::Blank,
    Step::Add("ba1", "2010-11-07 01:59:59 -0400", "PT1S"),
    Step::Add("ba2", "2010-11-06 04:30:00 -0400", "P1D"),
    Step::Add("ba3", "2010-11-06 04:30:00 -0400", "PT24H"),
    Step::Add("ba4", "2010-11-06 04:30:00 -0400", "PT23H"),
    Step::Add("ba5", "2010-11-06 04:30:00 -0400", "PT22H"),
    Step::Add("ba6", "2010-11-06 04:30:00 -0400", "PT21H"),
    Step::Add("ba7", "2010-11-06 01:30:00 -0400", "P1D"),
    Step::Add("ba8", "2010-11-06 01:30:00 -0400", "P1DT1H"),
    Step::Add("ba9", "2010-11-06 04:30:00 -0400", "PT25H"),
    Step::Add("ba10", "2010-11-06 03:30:00 -0400", "P1D"),
    Step::Add("ba11", "2010-11-06 02:30:00 -0400", "P1D"),
    Step::Blank,
    Step::Sub("bs1", "2010-11-07 01:00:00 -0500", "PT1S"),
    Step::Sub("bs2", "2010-11-07 04:30:00 -0500", "P1D"),
    Step::Sub("bs3", "2010-11-07 03:30:00 -0500", "PT24H"),
    Step::Sub("bs4", "2010-11-07 02:30:00 -0500", "PT23H"),
    Step::Sub("bs5", "2010-11-07 01:30:00 -0500", "PT22H"),
    Step::Sub("bs6", "2010-11-07 01:30:00 -0400", "PT21H"),
    Step::Sub("bs7", "2010-11-07 01:30:00 -0400", "P1D"),
    Step::Sub("bs8", "2010-11-07 01:30:00 -0500", "P1DT1H"),
    Step::Sub("bs9", "2010-11-07 03:30:00 -0500", "P1D"),
    Step::Sub("bs10", "2010-11-07 02:30:00 -0500", "P1D"),
];
/// Steps of `rfc-datetime_and_daylight_saving_time-type3-fa.phpt`.
const RFC_TYPE3_FA: &[Step] = &[
    Step::Add("fa1", "2010-03-14 01:59:59", "PT1S"),
    Step::Add("fa2", "2010-03-13 04:30:00", "P1D"),
    Step::Add("fa2.5", "2010-03-13 04:30:00", "PT23H"),
    Step::Add("fa3", "2010-03-13 04:30:00", "PT22H"),
    Step::Add("fa4", "2010-03-13 04:30:00", "PT21H"),
    Step::Add("fa5", "2010-03-13 01:30:00", "P1D"),
    Step::Add("fa6", "2010-03-13 02:30:00", "P1D"),
];

/// ext/date/tests/DateTime_add-fall-type3-type3.phpt
#[test]
fn datetime_add_fall_type3_type3() {
    assert_text(
        &run_examine_diff(FALL_TYPE3_TYPE3, Show::Add),
        r#"test_time_fall_type3_prev_type3_prev: ADD: 2010-10-04 02:18:48 EDT + P+0Y1M2DT16H19M40S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_prev_type3_dt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_prev_type3_redodt: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_prev_type3_redost: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT7H36M16S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_prev_type3_st: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M0DT9H38M27S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_prev_type3_post: ADD: 2010-11-06 18:38:28 EDT + P+0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_dt_type3_prev: ADD: 2010-11-07 00:10:20 EDT + P-0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_dt_type3_dt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT0H5M15S = **2010-11-07 00:15:35 EDT**
test_time_fall_type3_dt_type3_redodt: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_dt_type3_redost: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT2H4M24S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_dt_type3_st: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M0DT4H6M35S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_dt_type3_post: ADD: 2010-11-07 00:10:20 EDT + P+0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EST**
test_time_fall_type3_redodt_type3_prev: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_redodt_type3_dt: ADD: 2010-11-07 01:12:33 EDT + P-0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_redodt_type3_redodt: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT0H3M2S = **2010-11-07 01:15:35 EDT**
test_time_fall_type3_redodt_type3_redost: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT1H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_redodt_type3_st: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M0DT3H4M22S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_redodt_type3_post: ADD: 2010-11-07 01:12:33 EDT + P+0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EST**
test_time_fall_type3_redost_type3_prev: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT7H36M16S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_redost_type3_dt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT2H4M24S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_redost_type3_redodt: ADD: 2010-11-07 01:14:44 EST + P-0Y0M0DT1H2M11S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_redost_type3_redost: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT0H2M10S = **2010-11-07 01:16:54 EST**
test_time_fall_type3_redost_type3_st: ADD: 2010-11-07 01:14:44 EST + P+0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_redost_type3_post: ADD: 2010-11-07 01:14:44 EST + P+0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_st_type3_prev: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT9H38M27S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_st_type3_dt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT4H6M35S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_st_type3_redodt: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT3H4M22S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_st_type3_redost: ADD: 2010-11-07 03:16:55 EST + P-0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_st_type3_st: ADD: 2010-11-07 03:16:55 EST + P+0Y0M0DT2H3M1S = **2010-11-07 05:19:56 EST**
test_time_fall_type3_st_type3_post: ADD: 2010-11-07 03:16:55 EST + P+0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_post_type3_prev: ADD: 2010-11-08 19:59:59 EST + P-0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EDT**
test_time_fall_type3_post_type3_dt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT20H49M39S = **2010-11-07 00:10:20 EDT**
test_time_fall_type3_post_type3_redodt: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT19H47M26S = **2010-11-07 01:12:33 EDT**
test_time_fall_type3_post_type3_redost: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type3_post_type3_st: ADD: 2010-11-08 19:59:59 EST + P-0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type3_post_type3_post: ADD: 2010-11-08 18:57:55 EST + P+0Y0M0DT1H2M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type3_dtsec_type3_stsec: ADD: 2010-11-07 01:59:59 EDT + P+0Y0M0DT0H0M1S = **2010-11-07 01:00:00 EST**
test_time_fall_type3_stsec_type3_dtsec: ADD: 2010-11-07 01:00:00 EST + P-0Y0M0DT0H0M1S = **2010-11-07 01:59:59 EDT**
"#,
    );
}

/// ext/date/tests/DateTime_add-spring-type3-type3.phpt
#[test]
fn datetime_add_spring_type3_type3() {
    assert_text(
        &run_examine_diff(SPRING_TYPE3_TYPE3, Show::Add),
        r#"test_time_spring_type3_prev_type3_prev: ADD: 2010-02-11 02:18:48 EST + P+0Y1M2DT16H19M40S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_prev_type3_st: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_prev_type3_dt: ADD: 2010-03-13 18:38:28 EST + P+0Y0M0DT7H38M27S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_prev_type3_post: ADD: 2010-03-13 18:38:28 EST + P+0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_st_type3_prev: ADD: 2010-03-14 00:10:20 EST + P-0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_st_type3_st: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT0H5M15S = **2010-03-14 00:15:35 EST**
test_time_spring_type3_st_type3_dt: ADD: 2010-03-14 00:10:20 EST + P+0Y0M0DT2H6M35S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_st_type3_post: ADD: 2010-03-14 00:10:20 EST + P+0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EDT**
test_time_spring_type3_dt_type3_prev: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT7H38M27S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_dt_type3_st: ADD: 2010-03-14 03:16:55 EDT + P-0Y0M0DT2H6M35S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_dt_type3_dt: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M0DT2H3M1S = **2010-03-14 05:19:56 EDT**
test_time_spring_type3_dt_type3_post: ADD: 2010-03-14 03:16:55 EDT + P+0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_post_type3_prev: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EST**
test_time_spring_type3_post_type3_st: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT18H49M39S = **2010-03-14 00:10:20 EST**
test_time_spring_type3_post_type3_dt: ADD: 2010-03-15 19:59:59 EDT + P-0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type3_post_type3_post: ADD: 2010-03-15 18:57:55 EDT + P+0Y0M0DT1H2M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type3_stsec_type3_dtsec: ADD: 2010-03-14 01:59:59 EST + P+0Y0M0DT0H0M1S = **2010-03-14 03:00:00 EDT**
test_time_spring_type3_dtsec_type3_stsec: ADD: 2010-03-14 03:00:00 EDT + P-0Y0M0DT0H0M1S = **2010-03-14 01:59:59 EST**
"#,
    );
}

/// ext/date/tests/DateTime_days-fall-type2-type3.phpt
#[test]
fn datetime_days_fall_type2_type3() {
    assert_text(
        &run_examine_diff(FALL_TYPE2_TYPE3, Show::Days),
        r#"test_time_fall_type2_prev_type3_prev: DAYS: **33**
test_time_fall_type2_prev_type3_dt: DAYS: **0**
test_time_fall_type2_prev_type3_redodt: DAYS: **0**
test_time_fall_type2_prev_type3_redost: DAYS: **0**
test_time_fall_type2_prev_type3_st: DAYS: **0**
test_time_fall_type2_prev_type3_post: DAYS: **2**
test_time_fall_type2_dt_type3_prev: DAYS: **0**
test_time_fall_type2_dt_type3_dt: DAYS: **0**
test_time_fall_type2_dt_type3_redodt: DAYS: **0**
test_time_fall_type2_dt_type3_redost: DAYS: **0**
test_time_fall_type2_dt_type3_st: DAYS: **0**
test_time_fall_type2_dt_type3_post: DAYS: **1**
test_time_fall_type2_redodt_type3_prev: DAYS: **0**
test_time_fall_type2_redodt_type3_dt: DAYS: **0**
test_time_fall_type2_redodt_type3_redodt: DAYS: **0**
test_time_fall_type2_redodt_type3_redost: DAYS: **0**
test_time_fall_type2_redodt_type3_st: DAYS: **0**
test_time_fall_type2_redodt_type3_post: DAYS: **1**
test_time_fall_type2_redost_type3_prev: DAYS: **0**
test_time_fall_type2_redost_type3_dt: DAYS: **0**
test_time_fall_type2_redost_type3_redodt: DAYS: **0**
test_time_fall_type2_redost_type3_redost: DAYS: **0**
test_time_fall_type2_redost_type3_st: DAYS: **0**
test_time_fall_type2_redost_type3_post: DAYS: **1**
test_time_fall_type2_st_type3_prev: DAYS: **0**
test_time_fall_type2_st_type3_dt: DAYS: **0**
test_time_fall_type2_st_type3_redodt: DAYS: **0**
test_time_fall_type2_st_type3_redost: DAYS: **0**
test_time_fall_type2_st_type3_st: DAYS: **0**
test_time_fall_type2_st_type3_post: DAYS: **1**
test_time_fall_type2_post_type3_prev: DAYS: **2**
test_time_fall_type2_post_type3_dt: DAYS: **1**
test_time_fall_type2_post_type3_redodt: DAYS: **1**
test_time_fall_type2_post_type3_redost: DAYS: **1**
test_time_fall_type2_post_type3_st: DAYS: **1**
test_time_fall_type2_post_type3_post: DAYS: **0**
test_time_fall_type2_dtsec_type3_stsec: DAYS: **0**
test_time_fall_type2_stsec_type3_dtsec: DAYS: **0**
"#,
    );
}

/// ext/date/tests/DateTime_days-spring-type2-type3.phpt
#[test]
fn datetime_days_spring_type2_type3() {
    assert_text(
        &run_examine_diff(SPRING_TYPE2_TYPE3, Show::Days),
        r#"test_time_spring_type2_prev_type3_prev: DAYS: **30**
test_time_spring_type2_prev_type3_st: DAYS: **0**
test_time_spring_type2_prev_type3_dt: DAYS: **0**
test_time_spring_type2_prev_type3_post: DAYS: **2**
test_time_spring_type2_st_type3_prev: DAYS: **0**
test_time_spring_type2_st_type3_st: DAYS: **0**
test_time_spring_type2_st_type3_dt: DAYS: **0**
test_time_spring_type2_st_type3_post: DAYS: **1**
test_time_spring_type2_dt_type3_prev: DAYS: **0**
test_time_spring_type2_dt_type3_st: DAYS: **0**
test_time_spring_type2_dt_type3_dt: DAYS: **0**
test_time_spring_type2_dt_type3_post: DAYS: **1**
test_time_spring_type2_post_type3_prev: DAYS: **2**
test_time_spring_type2_post_type3_st: DAYS: **1**
test_time_spring_type2_post_type3_dt: DAYS: **1**
test_time_spring_type2_post_type3_post: DAYS: **0**
test_time_spring_type2_stsec_type3_dtsec: DAYS: **0**
test_time_spring_type2_dtsec_type3_stsec: DAYS: **0**
"#,
    );
}

/// ext/date/tests/DateTime_diff-fall-type2-type3.phpt
#[test]
fn datetime_diff_fall_type2_type3() {
    assert_text(
        &run_examine_diff(FALL_TYPE2_TYPE3, Show::Diff),
        r#"test_time_fall_type2_prev_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-10-04 02:18:48 EDT = **P+0Y1M2DT16H19M40S**
test_time_fall_type2_prev_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT5H31M52S**
test_time_fall_type2_prev_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT6H34M5S**
test_time_fall_type2_prev_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT7H36M16S**
test_time_fall_type2_prev_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M0DT9H38M27S**
test_time_fall_type2_prev_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-06 18:38:28 EDT = **P+0Y0M2DT2H21M31S**
test_time_fall_type2_dt_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 00:10:20 EDT = **P-0Y0M0DT5H31M52S**
test_time_fall_type2_dt_type3_dt: DIFF: 2010-11-07 00:15:35 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT0H5M15S**
test_time_fall_type2_dt_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT1H2M13S**
test_time_fall_type2_dt_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT2H4M24S**
test_time_fall_type2_dt_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M0DT4H6M35S**
test_time_fall_type2_dt_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 00:10:20 EDT = **P+0Y0M1DT20H49M39S**
test_time_fall_type2_redodt_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT6H34M5S**
test_time_fall_type2_redodt_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:12:33 EDT = **P-0Y0M0DT1H2M13S**
test_time_fall_type2_redodt_type3_redodt: DIFF: 2010-11-07 01:15:35 EDT - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT0H3M2S**
test_time_fall_type2_redodt_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT1H2M11S**
test_time_fall_type2_redodt_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M0DT3H4M22S**
test_time_fall_type2_redodt_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:12:33 EDT = **P+0Y0M1DT19H47M26S**
test_time_fall_type2_redost_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT7H36M16S**
test_time_fall_type2_redost_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT2H4M24S**
test_time_fall_type2_redost_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 01:14:44 EST = **P-0Y0M0DT1H2M11S**
test_time_fall_type2_redost_type3_redost: DIFF: 2010-11-07 01:16:54 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT0H2M10S**
test_time_fall_type2_redost_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-07 01:14:44 EST = **P+0Y0M0DT2H2M11S**
test_time_fall_type2_redost_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 01:14:44 EST = **P+0Y0M1DT18H45M15S**
test_time_fall_type2_st_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT9H38M27S**
test_time_fall_type2_st_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT4H6M35S**
test_time_fall_type2_st_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-07 03:16:55 EST = **P-0Y0M0DT3H4M22S**
test_time_fall_type2_st_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-07 03:16:55 EST = **P-0Y0M0DT2H2M11S**
test_time_fall_type2_st_type3_st: DIFF: 2010-11-07 05:19:56 EST - 2010-11-07 03:16:55 EST = **P+0Y0M0DT2H3M1S**
test_time_fall_type2_st_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-07 03:16:55 EST = **P+0Y0M1DT16H43M4S**
test_time_fall_type2_post_type3_prev: DIFF: 2010-11-06 18:38:28 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M2DT2H21M31S**
test_time_fall_type2_post_type3_dt: DIFF: 2010-11-07 00:10:20 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT20H49M39S**
test_time_fall_type2_post_type3_redodt: DIFF: 2010-11-07 01:12:33 EDT - 2010-11-08 19:59:59 EST = **P-0Y0M1DT19H47M26S**
test_time_fall_type2_post_type3_redost: DIFF: 2010-11-07 01:14:44 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT18H45M15S**
test_time_fall_type2_post_type3_st: DIFF: 2010-11-07 03:16:55 EST - 2010-11-08 19:59:59 EST = **P-0Y0M1DT16H43M4S**
test_time_fall_type2_post_type3_post: DIFF: 2010-11-08 19:59:59 EST - 2010-11-08 18:57:55 EST = **P+0Y0M0DT1H2M4S**
test_time_fall_type2_dtsec_type3_stsec: DIFF: 2010-11-07 01:00:00 EST - 2010-11-07 01:59:59 EDT = **P+0Y0M0DT0H0M1S**
test_time_fall_type2_stsec_type3_dtsec: DIFF: 2010-11-07 01:59:59 EDT - 2010-11-07 01:00:00 EST = **P-0Y0M0DT0H0M1S**
"#,
    );
}

/// ext/date/tests/DateTime_diff-spring-type2-type3.phpt
#[test]
fn datetime_diff_spring_type2_type3() {
    assert_text(
        &run_examine_diff(SPRING_TYPE2_TYPE3, Show::Diff),
        r#"test_time_spring_type2_prev_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-02-11 02:18:48 EST = **P+0Y1M2DT16H19M40S**
test_time_spring_type2_prev_type3_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-13 18:38:28 EST = **P+0Y0M0DT5H31M52S**
test_time_spring_type2_prev_type3_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M0DT7H38M27S**
test_time_spring_type2_prev_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-13 18:38:28 EST = **P+0Y0M2DT0H21M31S**
test_time_spring_type2_st_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 00:10:20 EST = **P-0Y0M0DT5H31M52S**
test_time_spring_type2_st_type3_st: DIFF: 2010-03-14 00:15:35 EST - 2010-03-14 00:10:20 EST = **P+0Y0M0DT0H5M15S**
test_time_spring_type2_st_type3_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M0DT2H6M35S**
test_time_spring_type2_st_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 00:10:20 EST = **P+0Y0M1DT18H49M39S**
test_time_spring_type2_dt_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT7H38M27S**
test_time_spring_type2_dt_type3_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-14 03:16:55 EDT = **P-0Y0M0DT2H6M35S**
test_time_spring_type2_dt_type3_dt: DIFF: 2010-03-14 05:19:56 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M0DT2H3M1S**
test_time_spring_type2_dt_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-14 03:16:55 EDT = **P+0Y0M1DT16H43M4S**
test_time_spring_type2_post_type3_prev: DIFF: 2010-03-13 18:38:28 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M2DT0H21M31S**
test_time_spring_type2_post_type3_st: DIFF: 2010-03-14 00:10:20 EST - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT18H49M39S**
test_time_spring_type2_post_type3_dt: DIFF: 2010-03-14 03:16:55 EDT - 2010-03-15 19:59:59 EDT = **P-0Y0M1DT16H43M4S**
test_time_spring_type2_post_type3_post: DIFF: 2010-03-15 19:59:59 EDT - 2010-03-15 18:57:55 EDT = **P+0Y0M0DT1H2M4S**
test_time_spring_type2_stsec_type3_dtsec: DIFF: 2010-03-14 03:00:00 EDT - 2010-03-14 01:59:59 EST = **P+0Y0M0DT0H0M1S**
test_time_spring_type2_dtsec_type3_stsec: DIFF: 2010-03-14 01:59:59 EST - 2010-03-14 03:00:00 EDT = **P-0Y0M0DT0H0M1S**
"#,
    );
}

/// ext/date/tests/DateTime_sub-fall-type2-type3.phpt
#[test]
fn datetime_sub_fall_type2_type3() {
    assert_text(
        &run_examine_diff(FALL_TYPE2_TYPE3, Show::Sub),
        r#"test_time_fall_type2_prev_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P+0Y1M2DT16H19M40S = **2010-10-04 02:18:48 EDT**
test_time_fall_type2_prev_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P+0Y0M0DT5H31M52S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT6H34M5S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type3_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT7H36M16S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT9H38M27S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_prev_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M2DT1H21M31S = **2010-11-06 18:38:28 EDT**
test_time_fall_type2_dt_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT5H31M52S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type3_dt: SUB: 2010-11-07 00:15:35 EDT - P+0Y0M0DT0H5M15S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P+0Y0M0DT1H2M13S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type3_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT2H4M24S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT4H6M35S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_dt_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT20H49M39S = **2010-11-07 00:10:20 EDT**
test_time_fall_type2_redodt_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT6H34M5S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT1H2M13S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type3_redodt: SUB: 2010-11-07 01:15:35 EDT - P+0Y0M0DT0H3M2S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type3_redost: SUB: 2010-11-07 01:14:44 EST - P+0Y0M0DT1H2M11S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT3H4M22S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redodt_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT19H47M26S = **2010-11-07 01:12:33 EDT**
test_time_fall_type2_redost_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT7H36M16S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_redost_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT2H4M24S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_redost_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT1H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_redost_type3_redost: SUB: 2010-11-07 01:16:54 EST - P+0Y0M0DT0H2M10S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_redost_type3_st: SUB: 2010-11-07 03:16:55 EST - P+0Y0M0DT2H2M11S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_redost_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT18H45M15S = **2010-11-07 01:14:44 EST**
test_time_fall_type2_st_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M0DT9H38M27S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_st_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M0DT4H6M35S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_st_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M0DT3H4M22S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_st_type3_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M0DT2H2M11S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_st_type3_st: SUB: 2010-11-07 05:19:56 EST - P+0Y0M0DT2H3M1S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_st_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M1DT16H43M4S = **2010-11-07 03:16:55 EST**
test_time_fall_type2_post_type3_prev: SUB: 2010-11-06 18:38:28 EDT - P-0Y0M2DT1H21M31S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_post_type3_dt: SUB: 2010-11-07 00:10:20 EDT - P-0Y0M1DT20H49M39S = **2010-11-08 20:59:59 EST**
test_time_fall_type2_post_type3_redodt: SUB: 2010-11-07 01:12:33 EDT - P-0Y0M1DT19H47M26S = **2010-11-08 20:59:59 EST**
test_time_fall_type2_post_type3_redost: SUB: 2010-11-07 01:14:44 EST - P-0Y0M1DT18H45M15S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_post_type3_st: SUB: 2010-11-07 03:16:55 EST - P-0Y0M1DT16H43M4S = **2010-11-08 19:59:59 EST**
test_time_fall_type2_post_type3_post: SUB: 2010-11-08 19:59:59 EST - P+0Y0M0DT1H2M4S = **2010-11-08 18:57:55 EST**
test_time_fall_type2_dtsec_type3_stsec: SUB: 2010-11-07 01:00:00 EST - P+0Y0M0DT0H0M1S = **2010-11-07 01:59:59 EDT**
test_time_fall_type2_stsec_type3_dtsec: SUB: 2010-11-07 01:59:59 EDT - P-0Y0M0DT0H0M1S = **2010-11-07 01:00:00 EST**
"#,
    );
}

/// ext/date/tests/DateTime_sub-spring-type2-type3.phpt
#[test]
fn datetime_sub_spring_type2_type3() {
    assert_text(
        &run_examine_diff(SPRING_TYPE2_TYPE3, Show::Sub),
        r#"test_time_spring_type2_prev_type3_prev: SUB: 2010-03-13 18:38:28 EST - P+0Y1M2DT16H19M40S = **2010-02-11 02:18:48 EST**
test_time_spring_type2_prev_type3_st: SUB: 2010-03-14 00:10:20 EST - P+0Y0M0DT5H31M52S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_prev_type3_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT7H38M27S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_prev_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M2DT1H21M31S = **2010-03-13 18:38:28 EST**
test_time_spring_type2_st_type3_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT5H31M52S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_st_type3_st: SUB: 2010-03-14 00:15:35 EST - P+0Y0M0DT0H5M15S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_st_type3_dt: SUB: 2010-03-14 03:16:55 EDT - P+0Y0M0DT2H6M35S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_st_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT18H49M39S = **2010-03-14 00:10:20 EST**
test_time_spring_type2_dt_type3_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M0DT7H38M27S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_dt_type3_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M0DT2H6M35S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_dt_type3_dt: SUB: 2010-03-14 05:19:56 EDT - P+0Y0M0DT2H3M1S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_dt_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M1DT16H43M4S = **2010-03-14 03:16:55 EDT**
test_time_spring_type2_post_type3_prev: SUB: 2010-03-13 18:38:28 EST - P-0Y0M2DT1H21M31S = **2010-03-15 19:59:59 EDT**
test_time_spring_type2_post_type3_st: SUB: 2010-03-14 00:10:20 EST - P-0Y0M1DT18H49M39S = **2010-03-15 18:59:59 EDT**
test_time_spring_type2_post_type3_dt: SUB: 2010-03-14 03:16:55 EDT - P-0Y0M1DT16H43M4S = **2010-03-15 19:59:59 EDT**
test_time_spring_type2_post_type3_post: SUB: 2010-03-15 19:59:59 EDT - P+0Y0M0DT1H2M4S = **2010-03-15 18:57:55 EDT**
test_time_spring_type2_stsec_type3_dtsec: SUB: 2010-03-14 03:00:00 EDT - P+0Y0M0DT0H0M1S = **2010-03-14 01:59:59 EST**
test_time_spring_type2_dtsec_type3_stsec: SUB: 2010-03-14 01:59:59 EST - P-0Y0M0DT0H0M1S = **2010-03-14 03:00:00 EDT**
"#,
    );
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type1.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type1() {
    assert_text(
        &run_steps(RFC_TYPE1, "Y-m-d H:i:s e"),
        r#"fd1 2010-03-14 03:00:00 -04:00 - 2010-03-14 01:59:59 -05:00 = PT0H0M1S
fd2 2010-03-14 04:30:00 -04:00 - 2010-03-13 04:30:00 -05:00 = P0DT23H
fd3 2010-03-14 03:30:00 -04:00 - 2010-03-13 04:30:00 -05:00 = P0DT22H
fd4 2010-03-14 01:30:00 -05:00 - 2010-03-13 04:30:00 -05:00 = P0DT21H
fd5 2010-03-14 01:30:00 -05:00 - 2010-03-13 01:30:00 -05:00 = P1DT0H
fd6 2010-03-14 03:30:00 -04:00 - 2010-03-13 03:30:00 -05:00 = P0DT23H
fd7 2010-03-14 03:30:00 -04:00 - 2010-03-13 02:30:00 -05:00 = P1DT0H

fa1 2010-03-14 01:59:59 -05:00 + PT1S = 2010-03-14 02:00:00 -05:00
fa2 2010-03-13 04:30:00 -05:00 + P1D = 2010-03-14 04:30:00 -05:00
fa3 2010-03-13 04:30:00 -05:00 + PT22H = 2010-03-14 02:30:00 -05:00
fa4 2010-03-13 04:30:00 -05:00 + PT21H = 2010-03-14 01:30:00 -05:00
fa5 2010-03-13 01:30:00 -05:00 + P1D = 2010-03-14 01:30:00 -05:00
fa6 2010-03-13 02:30:00 -05:00 + P1D = 2010-03-14 02:30:00 -05:00

fs1 2010-03-14 03:00:00 -04:00 - PT1S = 2010-03-14 02:59:59 -04:00
fs2 2010-03-14 04:30:00 -04:00 - P1D = 2010-03-13 04:30:00 -04:00
fs3 2010-03-14 03:30:00 -04:00 - PT22H = 2010-03-13 05:30:00 -04:00
fs4 2010-03-14 01:30:00 -05:00 - PT21H = 2010-03-13 04:30:00 -05:00
fs5 2010-03-14 01:30:00 -05:00 - P1D = 2010-03-13 01:30:00 -05:00
fs6 2010-03-15 03:30:00 -04:00 - P1D = 2010-03-14 03:30:00 -04:00
fs7 2010-03-15 02:30:00 -04:00 - P1D = 2010-03-14 02:30:00 -04:00

bd1 2010-11-07 01:00:00 -05:00 - 2010-11-07 01:59:59 -04:00 = PT0H0M1S
bd2 2010-11-07 04:30:00 -05:00 - 2010-11-06 04:30:00 -04:00 = P1DT1H
bd3 2010-11-07 03:30:00 -05:00 - 2010-11-06 04:30:00 -04:00 = P1DT0H
bd4 2010-11-07 02:30:00 -05:00 - 2010-11-06 04:30:00 -04:00 = P0DT23H
bd5 2010-11-07 01:30:00 -05:00 - 2010-11-06 04:30:00 -04:00 = P0DT22H
bd6 2010-11-07 01:30:00 -04:00 - 2010-11-06 04:30:00 -04:00 = P0DT21H
bd7 2010-11-07 01:30:00 -04:00 - 2010-11-06 01:30:00 -04:00 = P1DT0H
bd8 2010-11-07 01:30:00 -05:00 - 2010-11-06 01:30:00 -04:00 = P1DT1H

ba1 2010-11-07 01:59:59 -04:00 + PT1S = 2010-11-07 02:00:00 -04:00
ba2 2010-11-06 04:30:00 -04:00 + P1D = 2010-11-07 04:30:00 -04:00
ba3 2010-11-06 04:30:00 -04:00 + PT24H = 2010-11-07 04:30:00 -04:00
ba4 2010-11-06 04:30:00 -04:00 + PT23H = 2010-11-07 03:30:00 -04:00
ba5 2010-11-06 04:30:00 -04:00 + PT22H = 2010-11-07 02:30:00 -04:00
ba6 2010-11-06 04:30:00 -04:00 + PT21H = 2010-11-07 01:30:00 -04:00
ba7 2010-11-06 01:30:00 -04:00 + P1D = 2010-11-07 01:30:00 -04:00
ba8 2010-11-06 01:30:00 -04:00 + P1DT1H = 2010-11-07 02:30:00 -04:00
ba9 2010-11-06 04:30:00 -04:00 + PT25H = 2010-11-07 05:30:00 -04:00
ba10 2010-11-06 03:30:00 -04:00 + P1D = 2010-11-07 03:30:00 -04:00
ba11 2010-11-06 02:30:00 -04:00 + P1D = 2010-11-07 02:30:00 -04:00

bs1 2010-11-07 01:00:00 -05:00 - PT1S = 2010-11-07 00:59:59 -05:00
bs2 2010-11-07 04:30:00 -05:00 - P1D = 2010-11-06 04:30:00 -05:00
bs3 2010-11-07 03:30:00 -05:00 - PT24H = 2010-11-06 03:30:00 -05:00
bs4 2010-11-07 02:30:00 -05:00 - PT23H = 2010-11-06 03:30:00 -05:00
bs5 2010-11-07 01:30:00 -05:00 - PT22H = 2010-11-06 03:30:00 -05:00
bs6 2010-11-07 01:30:00 -04:00 - PT21H = 2010-11-06 04:30:00 -04:00
bs7 2010-11-07 01:30:00 -04:00 - P1D = 2010-11-06 01:30:00 -04:00
bs8 2010-11-07 01:30:00 -05:00 - P1DT1H = 2010-11-06 00:30:00 -05:00
bs9 2010-11-07 03:30:00 -05:00 - P1D = 2010-11-06 03:30:00 -05:00
bs10 2010-11-07 02:30:00 -05:00 - P1D = 2010-11-06 02:30:00 -05:00
"#,
    );
}

/// ext/date/tests/rfc-datetime_and_daylight_saving_time-type3-fa.phpt
#[test]
fn rfc_datetime_and_daylight_saving_time_type3_fa() {
    assert_text(
        &run_steps(RFC_TYPE3_FA, "Y-m-d H:i:s T e"),
        r#"fa1 2010-03-14 01:59:59 EST America/New_York + PT1S = 2010-03-14 03:00:00 EDT America/New_York
fa2 2010-03-13 04:30:00 EST America/New_York + P1D = 2010-03-14 04:30:00 EDT America/New_York
fa2.5 2010-03-13 04:30:00 EST America/New_York + PT23H = 2010-03-14 04:30:00 EDT America/New_York
fa3 2010-03-13 04:30:00 EST America/New_York + PT22H = 2010-03-14 03:30:00 EDT America/New_York
fa4 2010-03-13 04:30:00 EST America/New_York + PT21H = 2010-03-14 01:30:00 EST America/New_York
fa5 2010-03-13 01:30:00 EST America/New_York + P1D = 2010-03-14 01:30:00 EST America/New_York
fa6 2010-03-13 02:30:00 EST America/New_York + P1D = 2010-03-14 03:30:00 EDT America/New_York
"#,
    );
}
