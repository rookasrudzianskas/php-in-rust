//! Unit tests ported from `ext/calendar/tests/*.phpt`.
//!
//! The 32-bit-only phpt files (`bug53574_1`, `bug55797_1`, `bug80185_32bit`,
//! `easter_date_32bit`, `easter_date_checks_upper_bound_32bit`,
//! `jdtogregorian_overflow`) are skipped by php-src on a 64-bit build and are
//! not ported; this port is 64-bit only.

use php_runtime::Value;
use php_timelib::{builtin_tzinfo, format_timestamp};

use super::easter::timestamp_in;
use super::french::{french_to_sdn, sdn_to_french};
use super::gregor::{gregorian_to_sdn, sdn_to_gregorian};
use super::jewish::{jewish_to_sdn, sdn_to_jewish};
use super::julian::{julian_to_sdn, sdn_to_julian};
use super::localtime::{libc_timezone, zone_from_name};
use super::{CAL_FRENCH, CalDate, heb_number_to_chars};
use crate::testing::{TestVm, thrown};

macro_rules! phpt {
    ($name:literal) => {
        include_str!(concat!(
            "../../../../tests/phpt/ext/calendar/tests/",
            $name,
            ".phpt"
        ))
    };
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// The `--EXPECT--` / `--EXPECTF--` section of a phpt (some files use CRLF line
/// endings), trailing whitespace trimmed.
fn expected(phpt: &str) -> String {
    let phpt = phpt.replace("\r\n", "\n");
    let (marker, len) = if phpt.contains("--EXPECT--\n") {
        ("--EXPECT--\n", 11)
    } else {
        ("--EXPECTF--\n", 12)
    };
    let start = phpt.find(marker).expect("expect section") + len;
    phpt[start..].trim_end().to_string()
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn ints(args: &[i64]) -> Vec<Value> {
    args.iter().map(|&n| Value::Int(n)).collect()
}

/// A TestVm whose C library timezone is UTC (the `putenv('TZ=UTC')` of the phpt files).
fn vm_utc() -> TestVm {
    let mut t = TestVm::new();
    t.call("putenv", vec![Value::str("TZ=UTC")])
        .expect("putenv");
    t
}

fn call(t: &mut TestVm, f: &str, args: &[i64]) -> Value {
    t.call(f, ints(args))
        .unwrap_or_else(|_| panic!("{f}{args:?} threw"))
}

fn int(t: &mut TestVm, f: &str, args: &[i64]) -> i64 {
    match call(t, f, args) {
        Value::Int(n) => n,
        other => panic!("{f}{args:?} returned {other:?}"),
    }
}

fn bytes(t: &mut TestVm, f: &str, args: Vec<Value>) -> Vec<u8> {
    match t.call(f, args).expect("no exception") {
        Value::String(s) => s.as_bytes().to_vec(),
        other => panic!("{f} returned {other:?}"),
    }
}

fn text(t: &mut TestVm, f: &str, args: &[i64]) -> String {
    String::from_utf8(bytes(t, f, ints(args))).expect("ascii result")
}

/// `ValueError` thrown by a call: `"ValueError: message"`, like the phpt scripts print.
fn error(t: &mut TestVm, f: &str, args: &[i64]) -> String {
    let (class, message) = thrown(t.call(f, ints(args)));
    format!("{class}: {message}")
}

/// `var_dump()` of null/bool/int/string/array values.
fn var_dump(v: &Value) -> String {
    fn go(v: &Value, indent: usize, out: &mut String) {
        let pad = " ".repeat(indent);
        match v {
            Value::Null => out.push_str(&format!("{pad}NULL\n")),
            Value::Bool(b) => out.push_str(&format!("{pad}bool({b})\n")),
            Value::Int(n) => out.push_str(&format!("{pad}int({n})\n")),
            Value::String(s) => out.push_str(&format!(
                "{pad}string({}) \"{}\"\n",
                s.len(),
                s.to_string_lossy()
            )),
            Value::Array(a) => {
                out.push_str(&format!("{pad}array({}) {{\n", a.len()));
                for (k, v) in a.iter() {
                    match k.as_int() {
                        Some(n) => out.push_str(&format!("{pad}  [{n}]=>\n")),
                        None => out.push_str(&format!(
                            "{pad}  [\"{}\"]=>\n",
                            k.as_str().expect("string key").to_string_lossy()
                        )),
                    }
                    go(v, indent + 2, out);
                }
                out.push_str(&format!("{pad}}}\n"));
            }
            other => panic!("unsupported {other:?}"),
        }
    }
    let mut out = String::new();
    go(v, 0, &mut out);
    out
}

/// `print_r()` of nested arrays of ints and strings.
fn print_r(v: &Value) -> String {
    fn go(v: &Value, indent: usize, out: &mut String) {
        match v {
            Value::Int(n) => out.push_str(&n.to_string()),
            Value::String(s) => out.push_str(&s.to_string_lossy()),
            Value::Array(a) => {
                let pad = " ".repeat(indent);
                out.push_str(&format!("Array\n{pad}(\n"));
                for (k, v) in a.iter() {
                    let key = match k.as_int() {
                        Some(n) => n.to_string(),
                        None => k
                            .as_str()
                            .expect("string key")
                            .to_string_lossy()
                            .into_owned(),
                    };
                    out.push_str(&format!("{pad}    [{key}] => "));
                    go(v, indent + 8, out);
                    out.push('\n');
                }
                out.push_str(&format!("{pad})\n"));
            }
            other => panic!("unsupported {other:?}"),
        }
    }
    let mut out = String::new();
    go(v, 0, &mut out);
    out
}

fn utc_date(ts: i64) -> String {
    let utc = builtin_tzinfo("UTC").expect("UTC");
    String::from_utf8(format_timestamp(b"Y-m-d", ts, Some(&utc))).expect("ascii")
}

fn assert_val(got: &Value, want: &Value) {
    assert_eq!(var_dump(got), var_dump(want));
}

fn assoc<'a>(v: &'a Value, key: &str) -> &'a Value {
    let Value::Array(a) = v else {
        panic!("array expected, got {v:?}")
    };
    a.get_str(key.as_bytes())
        .unwrap_or_else(|| panic!("missing key {key}"))
}

// ---------------------------------------------------------------------------
// registration
// ---------------------------------------------------------------------------

#[test]
fn functions_and_constants_are_registered() {
    let t = TestVm::new();
    for f in [
        "cal_days_in_month",
        "cal_from_jd",
        "cal_info",
        "cal_to_jd",
        "easter_date",
        "easter_days",
        "frenchtojd",
        "gregoriantojd",
        "jddayofweek",
        "jdmonthname",
        "jdtofrench",
        "jdtogregorian",
        "jdtojewish",
        "jdtojulian",
        "jdtounix",
        "jewishtojd",
        "juliantojd",
        "unixtojd",
    ] {
        assert!(t.vm.find_function(f).is_some(), "{f} missing");
    }
    for (name, value) in [
        ("CAL_GREGORIAN", 0),
        ("CAL_JULIAN", 1),
        ("CAL_JEWISH", 2),
        ("CAL_FRENCH", 3),
        ("CAL_NUM_CALS", 4),
        ("CAL_DOW_DAYNO", 0),
        ("CAL_DOW_LONG", 1),
        ("CAL_DOW_SHORT", 2),
        ("CAL_MONTH_GREGORIAN_SHORT", 0),
        ("CAL_MONTH_GREGORIAN_LONG", 1),
        ("CAL_MONTH_JULIAN_SHORT", 2),
        ("CAL_MONTH_JULIAN_LONG", 3),
        ("CAL_MONTH_JEWISH", 4),
        ("CAL_MONTH_FRENCH", 5),
        ("CAL_EASTER_DEFAULT", 0),
        ("CAL_EASTER_ROMAN", 1),
        ("CAL_EASTER_ALWAYS_GREGORIAN", 2),
        ("CAL_EASTER_ALWAYS_JULIAN", 3),
        ("CAL_JEWISH_ADD_ALAFIM_GERESH", 2),
        ("CAL_JEWISH_ADD_ALAFIM", 4),
        ("CAL_JEWISH_ADD_GERESHAYIM", 8),
    ] {
        assert_eq!(
            t.vm.constant(name).and_then(|v| v.as_int()),
            Some(value),
            "{name}"
        );
    }
}

// ---------------------------------------------------------------------------
// SDN conversions
// ---------------------------------------------------------------------------

/// SDN -> date -> SDN is the identity on the supported range (the algorithms
/// are inverse by construction, a port slip breaks this immediately).
#[test]
fn sdn_round_trips() {
    for sdn in (1..=3_500_000).step_by(3) {
        let g = sdn_to_gregorian(sdn);
        assert_eq!(
            gregorian_to_sdn(g.year.into(), g.month.into(), g.day.into()),
            sdn,
            "gregorian {g:?}"
        );
        let j = sdn_to_julian(sdn);
        assert_eq!(
            julian_to_sdn(j.year.into(), j.month.into(), j.day.into()),
            sdn,
            "julian {j:?}"
        );
    }
    for sdn in 2_375_840..=2_380_952 {
        let f = sdn_to_french(sdn);
        assert_eq!(
            french_to_sdn(f.year.into(), f.month.into(), f.day.into()),
            sdn,
            "french {f:?}"
        );
    }
    for sdn in (347_998..=3_500_000).step_by(7) {
        let h = sdn_to_jewish(sdn);
        assert!(h.year > 0 && (1..=13).contains(&h.month) && (1..=30).contains(&h.day));
        assert_eq!(
            jewish_to_sdn(h.year.into(), h.month.into(), h.day.into()),
            sdn,
            "jewish {h:?}"
        );
    }
}

#[test]
fn sdn_known_dates() {
    // 2000-01-01 (Gregorian) = JD 2451545 = 1999-12-19 (Julian).
    assert_eq!(
        sdn_to_gregorian(2_451_545),
        CalDate {
            year: 2000,
            month: 1,
            day: 1
        }
    );
    assert_eq!(
        sdn_to_julian(2_451_545),
        CalDate {
            year: 1999,
            month: 12,
            day: 19
        }
    );
    // Rosh Hashanah 5784 was 2023-09-16, Pesach 5784 (a leap year) 2024-04-23,
    // Rosh Hashanah 5785 2024-10-03.
    let jd = |y, m, d| gregorian_to_sdn(y, m, d);
    assert_eq!(
        sdn_to_jewish(jd(2023, 9, 16)),
        CalDate {
            year: 5784,
            month: 1,
            day: 1
        }
    );
    assert_eq!(
        sdn_to_jewish(jd(2024, 4, 23)),
        CalDate {
            year: 5784,
            month: 8,
            day: 15
        }
    );
    assert_eq!(
        sdn_to_jewish(jd(2024, 10, 3)),
        CalDate {
            year: 5785,
            month: 1,
            day: 1
        }
    );
    // The French Republic: 22 September 1792 = 1 Vendemiaire I.
    assert_eq!(
        sdn_to_french(jd(1792, 9, 22)),
        CalDate {
            year: 1,
            month: 1,
            day: 1
        }
    );
}

// ---------------------------------------------------------------------------
// cal_days_in_month
// ---------------------------------------------------------------------------

#[test]
fn cal_days_in_month_phpt() {
    let mut t = TestVm::new();
    // cal_days_in_month.phpt
    let mut out = String::new();
    for (month, year, label) in [
        (8, 2003, "August 2003"),
        (2, 2003, "February 2003"),
        (2, 2004, "February 2004"),
        (12, 2034, "December 2034"),
    ] {
        let num = int(&mut t, "cal_days_in_month", &[0, month, year]);
        out.push_str(&format!("There are {num} days in {label}\n"));
    }
    assert_eq!(out.trim_end(), expected(phpt!("cal_days_in_month")));

    // bug52744.phpt: December 1 BCE.
    assert_eq!(int(&mut t, "cal_days_in_month", &[0, 12, -1]), 31);
    assert_eq!(int(&mut t, "cal_days_in_month", &[1, 12, -1]), 31);

    // bug67976.phpt: the final month of the French calendar.
    assert_eq!(int(&mut t, "cal_days_in_month", &[3, 13, 14]), 5);
}

#[test]
fn cal_days_in_month_jewish_bug54254() {
    let mut t = TestVm::new();
    let mut got = Vec::new();
    for year in [5771, 5772] {
        for month in 1..=13 {
            got.push(format!(
                "int({})",
                int(&mut t, "cal_days_in_month", &[2, month, year])
            ));
        }
    }
    assert_eq!(got.join("\n"), expected(phpt!("bug54254")));
}

#[test]
fn cal_days_in_month_errors() {
    let mut t = TestVm::new();
    // cal_days_in_month_error1.phpt
    let got = format!(
        "{}\n{}",
        error(&mut t, "cal_days_in_month", &[-1, 4, 2017]),
        error(&mut t, "cal_days_in_month", &[0, 20, 2009])
    );
    assert_eq!(got, expected(phpt!("cal_days_in_month_error1")));

    // gh19371.phpt
    let (min, max) = (i64::MIN, i64::MAX);
    let got = [
        error(&mut t, "cal_days_in_month", &[0, 12, max]),
        error(&mut t, "cal_days_in_month", &[0, min, 1]),
        error(&mut t, "cal_days_in_month", &[0, max, 1]),
        error(&mut t, "cal_to_jd", &[0, min, 1, 1]),
        error(&mut t, "cal_to_jd", &[0, max, 1, 1]),
        error(&mut t, "cal_to_jd", &[0, 1, min, 1]),
        error(&mut t, "cal_to_jd", &[0, 1, max, 1]),
        error(&mut t, "cal_to_jd", &[0, 1, 1, max]),
    ];
    assert_eq!(got.join("\n"), expected(phpt!("gh19371")));
}

// ---------------------------------------------------------------------------
// cal_from_jd / cal_to_jd / cal_info
// ---------------------------------------------------------------------------

#[test]
fn cal_from_jd_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for (jd, cal) in [(1_748_326, 0), (1_748_324, 1), (374_867, 2), (0, 3)] {
        out.push_str(&print_r(&call(&mut t, "cal_from_jd", &[jd, cal])));
    }
    assert_eq!(out.trim_end(), expected(phpt!("cal_from_jd")));

    assert_eq!(
        error(&mut t, "cal_from_jd", &[1_748_326, -1]),
        expected(phpt!("cal_from_jd_error1"))
    );
}

#[test]
fn cal_from_jd_out_of_range() {
    let mut t = TestVm::new();
    // bug71894.phpt
    let got = format!(
        "{}{}",
        var_dump(&call(&mut t, "cal_from_jd", &[347_997, 2])),
        var_dump(&call(&mut t, "jdmonthname", &[347_997, 4]))
    );
    assert_eq!(got.trim_end(), expected(phpt!("bug71894")));

    // bug53574_2, bug55797_2 and gh16834 (the %d / %s fields depend on the
    // day number): out of range for the calendar, so all-zero dates.
    for (jd, cal) in [
        (3_315_881_921_229_094_912, 1),
        (9_223_372_036_854_743_639, 0),
        (76_545_676_543_223, 0),
    ] {
        let v = call(&mut t, "cal_from_jd", &[jd, cal]);
        assert_val(assoc(&v, "date"), &Value::str("0/0/0"));
        assert_val(assoc(&v, "month"), &i(0));
        assert_val(assoc(&v, "day"), &i(0));
        assert_val(assoc(&v, "year"), &i(0));
        assert_val(assoc(&v, "dow"), &i((jd % 7 + 8) % 7));
        assert_val(assoc(&v, "abbrevmonth"), &Value::str(""));
        assert_val(assoc(&v, "monthname"), &Value::str(""));
    }
    // bug53574_2 spells out the day of the week.
    let v = call(&mut t, "cal_from_jd", &[3_315_881_921_229_094_912, 1]);
    assert_val(assoc(&v, "dow"), &i(3));
    assert_val(assoc(&v, "abbrevdayname"), &Value::str("Wed"));
    assert_val(assoc(&v, "dayname"), &Value::str("Wednesday"));
}

#[test]
fn cal_from_jd_layout() {
    let mut t = TestVm::new();
    let v = call(&mut t, "cal_from_jd", &[2_440_588, 0]);
    let Value::Array(a) = &v else { panic!() };
    let keys: Vec<String> = a
        .keys()
        .map(|k| {
            k.as_str()
                .expect("string key")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        keys,
        [
            "date",
            "month",
            "day",
            "year",
            "dow",
            "abbrevdayname",
            "dayname",
            "abbrevmonth",
            "monthname"
        ]
    );
    assert_val(assoc(&v, "date"), &Value::str("1/1/1970"));
    assert_val(assoc(&v, "dayname"), &Value::str("Thursday"));
    // A Jewish date before year 1 has no day of the week or month name.
    let v = call(&mut t, "cal_from_jd", &[100, 2]);
    assert_val(assoc(&v, "dow"), &Value::Null);
    assert_val(assoc(&v, "dayname"), &Value::str(""));
}

#[test]
fn cal_to_jd_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for cal in 0..4 {
        out.push_str(&format!(
            "{}\n",
            int(&mut t, "cal_to_jd", &[cal, 8, 26, 74])
        ));
    }
    assert_eq!(out.trim_end(), expected(phpt!("cal_to_jd")));

    assert_eq!(
        error(&mut t, "cal_to_jd", &[-1, 8, 26, 74]),
        expected(phpt!("cal_to_jd_error1"))
    );

    // tojd_out_of_int_range.phpt: cal_to_jd() and cal_days_in_month().
    let bias = 1i64 << 32;
    assert_eq!(int(&mut t, "cal_to_jd", &[0, 1, 1, 1 - bias]), 0);
    assert_eq!(
        error(&mut t, "cal_days_in_month", &[0, 1, 1 - bias]),
        "ValueError: Invalid date"
    );
}

#[test]
fn cal_info_phpt() {
    let mut t = TestVm::new();
    let mut out = print_r(&t.call("cal_info", vec![]).expect("cal_info()"));
    out.push_str(&print_r(&call(&mut t, "cal_info", &[1])));
    out.push_str(&error(&mut t, "cal_info", &[99999]));
    out.push('\n');
    assert_eq!(out.trim_end(), expected(phpt!("cal_info")));

    // Explicit -1 is the same as no argument.
    assert_eq!(
        print_r(&call(&mut t, "cal_info", &[-1])),
        print_r(&t.call("cal_info", vec![]).expect("cal_info()"))
    );
}

// ---------------------------------------------------------------------------
// easter
// ---------------------------------------------------------------------------

#[test]
fn easter_date_64bit() {
    let mut t = vm_utc();
    // easter_date_64bit.phpt
    let mut out = String::new();
    for year in [2000, 2001, 2002, 2045, 2046, 2047] {
        let ts = int(&mut t, "easter_date", &[year]);
        out.push_str(&format!("{}\n", utc_date(ts)));
    }
    out.push_str(&error(&mut t, "easter_date", &[1492]));
    assert_eq!(out, expected(phpt!("easter_date_64bit")));

    // easter_date_checks_upper_bound_64bit.phpt
    assert_eq!(
        error(&mut t, "easter_date", &[293_274_701_009]),
        expected(phpt!("easter_date_checks_upper_bound_64bit"))
    );
    // 2,000,000,000 itself is accepted.
    assert!(matches!(
        t.call("easter_date", vec![i(2_000_000_000)]),
        Ok(Value::Int(_))
    ));
    assert_eq!(
        error(&mut t, "easter_date", &[2_000_000_001]),
        "ValueError: easter_date(): Argument #1 ($year) must be a year before 2.000.000.000 (inclusive)"
    );
}

#[test]
fn easter_date_uses_the_libc_timezone() {
    // mktime() of midnight in the zone, not of midnight UTC.
    let amsterdam = builtin_tzinfo("Europe/Amsterdam").expect("zone");
    assert_eq!(timestamp_in(2000, 0, &amsterdam), 956_440_800); // 2000-04-22 22:00 UTC
    let utc = builtin_tzinfo("UTC").expect("zone");
    assert_eq!(timestamp_in(2000, 0, &utc), 956_448_000);
    let new_york = builtin_tzinfo("America/New_York").expect("zone");
    assert_eq!(timestamp_in(2001, 0, &new_york), 987_307_200); // 2001-04-15 04:00 UTC

    assert_eq!(zone_from_name("Europe/Amsterdam").name, "Europe/Amsterdam");
    assert_eq!(zone_from_name(":Europe/Amsterdam").name, "Europe/Amsterdam");
    assert_eq!(
        zone_from_name("/usr/share/zoneinfo/Europe/Amsterdam").name,
        "Europe/Amsterdam"
    );
    // Unknown zones are UTC, as in glibc.
    assert_eq!(zone_from_name("No/Such_Zone").name, "UTC");
    assert_eq!(zone_from_name("").name, "UTC");
}

#[test]
fn easter_days_phpt() {
    let mut t = TestVm::new();
    // easter_days.phpt
    let mut out = String::new();
    for year in [1999, 1492, 1913] {
        out.push_str(&format!("{}\n", int(&mut t, "easter_days", &[year])));
    }
    assert_eq!(out.trim_end(), expected(phpt!("easter_days")));

    // Gregorian Easter: 2000-04-23, 2024-03-31, 2025-04-20.
    assert_eq!(int(&mut t, "easter_days", &[2000]), 33);
    assert_eq!(int(&mut t, "easter_days", &[2024]), 10);
    assert_eq!(int(&mut t, "easter_days", &[2025]), 30);
    // The calendar in use depends on the year and the mode: 1700 is Julian by
    // default (31 March Julian), Gregorian with CAL_EASTER_ROMAN and
    // CAL_EASTER_ALWAYS_GREGORIAN (11 April), and 2000 is Julian with
    // CAL_EASTER_ALWAYS_JULIAN (17 April Julian = 30 April Gregorian).
    assert_eq!(int(&mut t, "easter_days", &[1700, 0]), 10);
    assert_eq!(int(&mut t, "easter_days", &[1700, 1]), 21);
    assert_eq!(int(&mut t, "easter_days", &[1700, 2]), 21);
    assert_eq!(int(&mut t, "easter_days", &[2000, 3]), 27);
}

#[test]
fn easter_default_year_is_the_current_one() {
    let mut t = vm_utc();
    let year = std::str::from_utf8(&format_timestamp(
        b"Y",
        super::localtime::now(),
        Some(&builtin_tzinfo("UTC").expect("zone")),
    ))
    .expect("ascii")
    .parse::<i64>()
    .expect("year");
    let with = int(&mut t, "easter_days", &[year]);
    let without = match t.call("easter_days", vec![]).expect("easter_days()") {
        Value::Int(n) => n,
        other => panic!("{other:?}"),
    };
    assert_eq!(with, without);
    let with_null = match t.call("easter_days", vec![Value::Null]).expect("null year") {
        Value::Int(n) => n,
        other => panic!("{other:?}"),
    };
    assert_eq!(with, with_null);
}

#[test]
fn easter_year_range() {
    let mut t = TestVm::new();
    // gh16228.phpt
    let max = (i64::MAX / 5) * 4;
    let want =
        |f: &str| format!("ValueError: {f}(): Argument #1 ($year) must be between 1 and {max}");
    assert_eq!(
        error(&mut t, "easter_days", &[i64::MAX, 0]),
        want("easter_days")
    );
    assert_eq!(error(&mut t, "easter_days", &[-1, 0]), want("easter_days"));
    assert_eq!(
        error(&mut t, "easter_date", &[i64::MAX, 0]),
        want("easter_date")
    );
    assert_eq!(error(&mut t, "easter_days", &[0, 0]), want("easter_days"));
    // The largest year is accepted by easter_days() without overflowing.
    assert!(matches!(
        t.call("easter_days", vec![i(max)]),
        Ok(Value::Int(d)) if (1..=35).contains(&d)
    ));
    // Years before 1970 are only rejected by easter_date().
    assert!(matches!(
        t.call("easter_days", vec![i(1)]),
        Ok(Value::Int(_))
    ));
}

// ---------------------------------------------------------------------------
// frenchtojd / gregoriantojd / juliantojd / jewishtojd
// ---------------------------------------------------------------------------

#[test]
fn frenchtojd_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for args in [[-1, -1, -1], [0, 0, 0], [1, 1, 1], [14, 31, 15]] {
        out.push_str(&format!("{}\n", int(&mut t, "frenchtojd", &args)));
    }
    assert_eq!(out.trim_end(), expected(phpt!("frenchtojd")));
}

#[test]
fn gregoriantojd_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for args in [
        [0, 0, 0],
        [1, 1, 1582],
        [10, 5, 1582],
        [1, 1, 1970],
        [1, 1, 2999],
        [1, 1, -4714],
        [11, 24, -4714],
    ] {
        out.push_str(&format!("{}\n", int(&mut t, "gregoriantojd", &args)));
    }
    assert_eq!(out.trim_end(), expected(phpt!("gregoriantojd")));
    // The first valid day.
    assert_eq!(int(&mut t, "gregoriantojd", &[11, 25, -4714]), 1);

    // gregoriantojd_overflow.phpt
    assert_eq!(
        int(&mut t, "gregoriantojd", &[5, 5, 6_000_000]),
        expected(phpt!("gregoriantojd_overflow"))
            .parse::<i64>()
            .unwrap()
    );
}

#[test]
fn juliantojd_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for args in [
        [0, 0, 0],
        [1, 1, 1582],
        [10, 5, 1582],
        [1, 1, 1970],
        [1, 1, 2999],
        [1, 1, -4713],
    ] {
        out.push_str(&format!("{}\n", int(&mut t, "juliantojd", &args)));
    }
    assert_eq!(out.trim_end(), expected(phpt!("juliantojd")));
    assert_eq!(int(&mut t, "juliantojd", &[1, 2, -4713]), 1);

    // juliantojd_overflow.phpt: the largest accepted year is INT_MAX - 4800.
    let got = format!(
        "{}\n{}",
        int(&mut t, "juliantojd", &[5, 5, 2_147_478_847]),
        int(&mut t, "juliantojd", &[5, 5, 6_000_000_000])
    );
    assert_eq!(got, expected(phpt!("juliantojd_overflow")));
}

#[test]
fn gh22602_int_max_years() {
    let mut t = TestVm::new();
    let (max, min) = (i64::from(i32::MAX), i64::from(i32::MIN));
    assert_eq!(int(&mut t, "gregoriantojd", &[5, 5, max]), 0);
    assert_eq!(int(&mut t, "gregoriantojd", &[5, 5, min]), 0);
    assert_eq!(int(&mut t, "juliantojd", &[5, 5, 2_147_483_647]), 0);
    assert_eq!(int(&mut t, "juliantojd", &[5, 5, -2_147_483_647]), 0);
}

#[test]
fn jewishtojd_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for args in [[-1, -1, -1], [0, 0, 0], [1, 1, 1], [2, 22, 5763]] {
        out.push_str(&format!("{}\n", int(&mut t, "jewishtojd", &args)));
    }
    assert_eq!(out.trim_end(), expected(phpt!("jewishtojd")));
}

#[test]
fn jewishtojd_overflow_gh16234() {
    let mut t = TestVm::new();
    // gh16234.phpt, gh16234_2.phpt: must not crash or hang.
    int(&mut t, "jewishtojd", &[1_218_182_888, 1, 1_218_182_888]);
    int(&mut t, "jewishtojd", &[10, 6, 2_147_483_647]);
    // gh16234_2_64.phpt
    assert_eq!(
        error(&mut t, "jewishtojd", &[10, 6, i64::MIN]),
        "ValueError: jewishtojd(): Argument #3 ($year) must be between -2147483648 and 2147483647"
    );
}

#[test]
fn jewishtojd_years_from_6000_gh21557() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for yh in 5995..6005 {
        let rh = int(&mut t, "jewishtojd", &[1, 1, yh]);
        out.push_str(&format!("yh={yh} rh={rh}\n"));
    }
    assert_eq!(out.trim_end(), expected(phpt!("gh21557")));
}

#[test]
fn tojd_out_of_int_range() {
    // tojd_out_of_int_range.phpt
    let mut t = TestVm::new();
    let bias = 1i64 << 32;
    let mut out = String::new();
    for f in ["gregoriantojd", "juliantojd", "jewishtojd", "frenchtojd"] {
        out.push_str(&format!("{f}\n"));
        let mut dump = |t: &mut TestVm, args: [i64; 3]| {
            out.push_str(&var_dump(&call(t, f, &args)));
        };
        dump(&mut t, [1, 1, 1]);
        dump(&mut t, [1 + bias, 1, 1]);
        dump(&mut t, [1, 1 + bias, 1]);
        match t.call(f, ints(&[1, 1, 1 + bias])) {
            Ok(v) => out.push_str(&var_dump(&v)),
            Err(e) => {
                let (class, msg) = thrown(Err::<Value, _>(e));
                out.push_str(&format!("{class}: {msg}\n"));
            }
        }
    }
    out.push_str("cal_to_jd\n");
    out.push_str(&var_dump(&call(&mut t, "cal_to_jd", &[0, 1, 1, 1 - bias])));
    out.push_str(&error(&mut t, "cal_days_in_month", &[0, 1, 1 - bias]));
    out.push('\n');
    assert_eq!(out.trim_end(), expected(phpt!("tojd_out_of_int_range")));
}

// ---------------------------------------------------------------------------
// jdto* conversions
// ---------------------------------------------------------------------------

#[test]
fn jdtofrench_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for jd in [0, 2_375_840, 2_375_850, 2_375_940, 2_376_345, 2_385_940] {
        out.push_str(&format!("{}\n", text(&mut t, "jdtofrench", &[jd])));
    }
    assert_eq!(out.trim_end(), expected(phpt!("jdtofrench")));
}

#[test]
fn jdtogregorian_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for jd in [0, 1, 2_298_874, 2_299_151, 2_440_588, 2_816_423] {
        out.push_str(&format!("{}\n", text(&mut t, "jdtogregorian", &[jd])));
    }
    assert_eq!(out.trim_end(), expected(phpt!("jdtogregorian")));

    // gh16235.phpt: no overflow. On a 64-bit build the date is real.
    assert_eq!(
        text(&mut t, "jdtogregorian", &[536_838_867]),
        "10/18/1465102"
    );
    // Day numbers whose arithmetic would overflow give 0/0/0.
    assert_eq!(text(&mut t, "jdtogregorian", &[i64::MAX]), "0/0/0");
}

#[test]
fn jdtojulian_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for jd in [0, 1, 2_298_874, 2_299_151, 2_440_588, 2_816_423] {
        out.push_str(&format!("{}\n", text(&mut t, "jdtojulian", &[jd])));
    }
    assert_eq!(out.trim_end(), expected(phpt!("jdtojulian")));
}

#[test]
fn jddayofweek_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for jd in [2_440_588, 2_452_162, 2_453_926, -1000] {
        out.push_str(&format!("### JD {jd} ###\n"));
        for mode in [0, 1, 2] {
            out.push_str(&format!("--- mode {mode} ---\n"));
            for offset in 0..=7 {
                match call(&mut t, "jddayofweek", &[jd + offset, mode]) {
                    Value::Int(n) => out.push_str(&format!("{n}\n")),
                    Value::String(s) => out.push_str(&format!("{}\n", s.to_string_lossy())),
                    other => panic!("{other:?}"),
                }
            }
        }
    }
    assert_eq!(out.trim_end(), expected(phpt!("jddayofweek")));

    // The mode defaults to CAL_DOW_DAYNO; unknown modes behave the same.
    assert_eq!(int(&mut t, "jddayofweek", &[2_440_588]), 4);
    assert_eq!(int(&mut t, "jddayofweek", &[2_440_588, 99]), 4);
    // gh16258.phpt
    call(&mut t, "jddayofweek", &[i64::MAX, 1]);
    call(&mut t, "jddayofweek", &[i64::MIN, 1]);
}

#[test]
fn jdmonthname_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for jd in [2_440_588, 2_452_162, 2_453_926] {
        out.push_str(&format!("### JD {jd} ###\n"));
        for mode in 0..=6 {
            out.push_str(&format!("--- mode {mode} ---\n"));
            for offset in 0..=12 {
                out.push_str(&format!(
                    "{}\n",
                    text(&mut t, "jdmonthname", &[jd + offset * 30, mode])
                ));
            }
        }
    }
    assert_eq!(out.trim_end(), expected(phpt!("jdmonthname")));
}

#[test]
fn jdtomonthname_phpt() {
    let mut t = TestVm::new();
    let mut out = String::new();
    for jd in [2_453_396, 2_440_588, -1, 10_000_000] {
        out.push_str(&format!("=== {jd}\n"));
        for mode in 0..=5 {
            out.push_str(&var_dump(&call(&mut t, "jdmonthname", &[jd, mode])));
        }
        out.push('\n');
    }
    out.push_str("Done\n");
    assert_eq!(out.trim_end(), expected(phpt!("jdtomonthname")));
}

// ---------------------------------------------------------------------------
// jdtojewish
// ---------------------------------------------------------------------------

/// The expected bytes of jdtojewish.phpt (ISO-8859-8).
#[test]
fn jdtojewish_hebrew_phpt() {
    let mut t = TestVm::new();
    let jd = |t: &mut TestVm, m, d, y| int(t, "gregoriantojd", &[m, d, y]);
    let heb = |t: &mut TestVm, jd, flags| {
        bytes(
            t,
            "jdtojewish",
            vec![Value::Int(jd), Value::Bool(true), Value::Int(flags)],
        )
    };
    const GERESH: i64 = 2;
    const ALAFIM: i64 = 4;
    const GERESHAYIM: i64 = 8;

    let d1 = jd(&mut t, 10, 28, 2002);
    assert_eq!(text(&mut t, "jdtojewish", &[d1]), "2/22/5763");
    let cases: [(i64, &[u8]); 5] = [
        (0, b"\xEB\xE1 \xE7\xF9\xE5\xEF \xE4\xFA\xF9\xF1\xE2"),
        (GERESH, b"\xEB\xE1 \xE7\xF9\xE5\xEF \xE4'\xFA\xF9\xF1\xE2"),
        (
            ALAFIM,
            b"\xEB\xE1 \xE7\xF9\xE5\xEF \xE4 \xE0\xEC\xF4\xE9\xED \xFA\xF9\xF1\xE2",
        ),
        (
            GERESH + ALAFIM,
            b"\xEB\xE1 \xE7\xF9\xE5\xEF \xE4' \xE0\xEC\xF4\xE9\xED \xFA\xF9\xF1\xE2",
        ),
        (
            GERESHAYIM,
            b"\xEB\"\xE1 \xE7\xF9\xE5\xEF \xE4\xFA\xF9\xF1\"\xE2",
        ),
    ];
    for (flags, want) in cases {
        assert_eq!(heb(&mut t, d1, flags), want, "flags {flags}");
    }
    // The flags are ignored without `$hebrew`.
    let plain = bytes(
        &mut t,
        "jdtojewish",
        vec![Value::Int(d1), Value::Bool(false), Value::Int(GERESHAYIM)],
    );
    assert_eq!(plain, b"2/22/5763");

    let d2 = jd(&mut t, 10, 8, 2002);
    let cases: [(i64, &[u8]); 4] = [
        (GERESHAYIM, b"\xE1' \xE7\xF9\xE5\xEF \xE4\xFA\xF9\xF1\"\xE2"),
        (
            GERESHAYIM + GERESH,
            b"\xE1' \xE7\xF9\xE5\xEF \xE4'\xFA\xF9\xF1\"\xE2",
        ),
        (
            GERESHAYIM + ALAFIM,
            b"\xE1' \xE7\xF9\xE5\xEF \xE4 \xE0\xEC\xF4\xE9\xED \xFA\xF9\xF1\"\xE2",
        ),
        (
            GERESHAYIM + ALAFIM + GERESH,
            b"\xE1' \xE7\xF9\xE5\xEF \xE4' \xE0\xEC\xF4\xE9\xED \xFA\xF9\xF1\"\xE2",
        ),
    ];
    for (flags, want) in cases {
        assert_eq!(heb(&mut t, d2, flags), want, "flags {flags}");
    }

    let d3 = jd(&mut t, 3, 10, 2007);
    assert_eq!(text(&mut t, "jdtojewish", &[d3]), "7/20/5767");
    for (args, want) in [
        ([11, 5, 2002], "2/30/5763"),
        ([11, 29, 2004], "3/16/5765"),
        ([1, 1, 9998], "3/8/13758"),
    ] {
        let d = jd(&mut t, args[0], args[1], args[2]);
        assert_eq!(text(&mut t, "jdtojewish", &[d]), want);
    }
    let d = jd(&mut t, 1, 1, 9998);
    let (class, msg) = thrown(t.call("jdtojewish", vec![Value::Int(d), Value::Bool(true)]));
    assert_eq!(
        format!("{class}: {msg}"),
        "ValueError: Year out of range (0-9999)"
    );
    // Year 0 (before the Jewish epoch) is out of range as well.
    let (_, msg) = thrown(t.call("jdtojewish", vec![Value::Int(1), Value::Bool(true)]));
    assert_eq!(msg, "Year out of range (0-9999)");
}

#[test]
fn heb_number_to_chars_cases() {
    let hebrew = |n, fl| heb_number_to_chars(n, fl).expect("in range");
    assert_eq!(heb_number_to_chars(0, 0), None);
    assert_eq!(heb_number_to_chars(10_000, 0), None);
    assert_eq!(hebrew(1, 0), b"\xE0");
    // 15 and 16 are written tet-vav and tet-zain.
    assert_eq!(hebrew(15, 0), b"\xE8\xE5");
    assert_eq!(hebrew(16, 0), b"\xE8\xE6");
    assert_eq!(hebrew(400, 0), b"\xFA");
    assert_eq!(hebrew(800, 0), b"\xFA\xFA");
    assert_eq!(hebrew(9999, 0), b"\xE8\xFA\xFA\xF7\xF6\xE8");
    // Year 5 and year 5000 give the same letter: the thousands only add one.
    assert_eq!(hebrew(5, 0), b"\xE4");
    assert_eq!(hebrew(5000, 0), b"\xE4");
    // Gereshayim: a geresh after a single letter, a gershayim before the last
    // of several.
    assert_eq!(hebrew(5, 8), b"\xE4'");
    assert_eq!(hebrew(15, 8), b"\xE8\"\xE5");
    // The thousands letter does not count as part of the number.
    assert_eq!(hebrew(5000, 8), b"\xE4");
}

#[test]
fn jdtojewish_month_names_jdtojewish_hebrew_phpt() {
    // jdtojewish_hebrew.phpt: 13 months of a normal and of a leap year.
    let mut t = TestVm::new();
    let mut out = String::new();
    for year in 5000..=5001 {
        let leap = if year == 5001 { "leap" } else { "normal" };
        out.push_str(&format!("{leap} year {year}\n"));
        for month in 1..=13 {
            let jd = int(&mut t, "jewishtojd", &[month, 1, year]);
            let hebrew = bytes(
                &mut t,
                "jdtojewish",
                vec![Value::Int(jd), Value::Bool(true)],
            );
            let hex: String = hebrew.iter().map(|b| format!("{b:02x}")).collect();
            out.push_str(&format!("{hex}\n"));
        }
        out.push('\n');
    }
    assert_eq!(out.trim_end(), expected(phpt!("jdtojewish_hebrew")));
}

#[test]
fn jdtojewish_64bit_limits() {
    let mut t = TestVm::new();
    // jdtojewish64.phpt
    let mut out = String::new();
    for x in [
        38_245_310,
        324_542_846,
        324_542_847,
        9_223_372_036_854_743_639,
    ] {
        out.push_str(&var_dump(&call(&mut t, "jdtojewish", &[x])));
    }
    assert_eq!(out.trim_end(), expected(phpt!("jdtojewish64")));

    // jdtojewish_overflow.phpt
    let mut out = String::new();
    for i in 324_542_840..324_542_850 {
        out.push_str(&format!("{i}:{}\n", text(&mut t, "jdtojewish", &[i])));
    }
    out.push_str("DONE\n");
    assert_eq!(out.trim_end(), expected(phpt!("jdtojewish_overflow")));
}

// ---------------------------------------------------------------------------
// cal_unix.c
// ---------------------------------------------------------------------------

#[test]
fn jdtounix_phpt() {
    let mut t = TestVm::new();
    // jdtounix.phpt
    let mut out = String::new();
    for jd in [2_440_588, 2_452_162, 2_453_926] {
        let ts = int(&mut t, "jdtounix", &[jd]);
        out.push_str(&format!("{}\n", utc_date(ts)));
    }
    assert_eq!(out.trim_end(), expected(phpt!("jdtounix")));

    // bug80185.phpt
    assert_eq!(int(&mut t, "jdtounix", &[2_465_712]), 2_170_713_600);
    assert_eq!(
        int(&mut t, "jdtounix", &[i64::MAX / 86400 + 2_440_588]),
        9_223_372_036_854_720_000
    );
    let limit = "ValueError: jdtounix(): Argument #1 ($julian_day) jday must be between 2440588 and 106751993607888";
    assert_eq!(
        error(&mut t, "jdtounix", &[i64::MAX / 86400 + 2_440_589]),
        limit
    );

    // jdtounix_error1.phpt, gh16231.phpt
    assert_eq!(error(&mut t, "jdtounix", &[2_440_579]), limit);
    assert_eq!(error(&mut t, "jdtounix", &[i64::MIN]), limit);
    assert_eq!(error(&mut t, "jdtounix", &[240_587]), limit);
}

#[test]
fn unixtojd_phpt() {
    let mut t = vm_utc();
    // unixtojd.phpt
    let mut out = String::new();
    // unixtojd() is the current day: a positive int.
    assert!(int(&mut t, "unixtojd", &[]) > 2_440_588);
    for ts in [40_000, 1_000_000_000, 1_152_459_009] {
        out.push_str(&format!("{}\n", int(&mut t, "unixtojd", &[ts])));
    }
    assert_eq!(out.trim_end(), "2440588\n2452162\n2453926");

    // unixtojd_error1.phpt
    assert_eq!(
        error(&mut t, "unixtojd", &[-1]),
        "ValueError: unixtojd(): Argument #1 ($timestamp) must be greater than or equal to 0"
    );
    assert!(matches!(
        t.call("unixtojd", vec![Value::Null]),
        Ok(Value::Int(_))
    ));
    let now = super::localtime::now();
    assert!(matches!(
        t.call("unixtojd", vec![i(now)]),
        Ok(Value::Int(_))
    ));
    // A timestamp whose year does not fit `tm_year`: localtime_r() fails.
    assert_val(
        &t.call("unixtojd", vec![i(i64::MAX)]).expect("no exception"),
        &Value::Bool(false),
    );
}

#[test]
fn unixtojd_follows_the_tz_environment_variable() {
    // The local date, not the UTC one: 1970-01-01 20:00 UTC is already the
    // next day in Asia/Tokyo.
    let tokyo = zone_from_name("Asia/Tokyo");
    let local = super::localtime::localtime(&tokyo, 20 * 3600).expect("date");
    assert_eq!((local.tm_year, local.tm_mon, local.tm_mday), (70, 0, 2));
    let utc = zone_from_name("UTC");
    let local = super::localtime::localtime(&utc, 20 * 3600).expect("date");
    assert_eq!((local.tm_year, local.tm_mon, local.tm_mday), (70, 0, 1));
    // `libc_timezone()` always resolves to some zone.
    assert!(!libc_timezone().name.is_empty());
}

#[test]
fn calendar_constants_match_the_conversion_table() {
    // `cal_info()` entry n describes calendar CAL_* n.
    let mut t = TestVm::new();
    for (cal, name, symbol) in [
        (0, "Gregorian", "CAL_GREGORIAN"),
        (1, "Julian", "CAL_JULIAN"),
        (2, "Jewish", "CAL_JEWISH"),
        (CAL_FRENCH, "French", "CAL_FRENCH"),
    ] {
        let info = call(&mut t, "cal_info", &[cal]);
        assert_val(assoc(&info, "calname"), &Value::str(name));
        assert_val(assoc(&info, "calsymbol"), &Value::str(symbol));
        assert_eq!(t.vm.constant(symbol).and_then(|v| v.as_int()), Some(cal));
    }
}
