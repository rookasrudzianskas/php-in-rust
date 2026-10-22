//! Port of `ext/calendar/easter.c`: `easter_date()` and `easter_days()`.

use std::sync::Arc;

use php_runtime::{Args, NativeFunction, PResult, Value, Vm};
use php_timelib::TzInfo;

use super::localtime::{libc_timezone, localtime, mktime_midnight, now};
use super::{CAL_EASTER_ALWAYS_GREGORIAN, CAL_EASTER_ALWAYS_JULIAN, CAL_EASTER_ROMAN};

/// Easter as the number of days after March 21st (`easter` in `_cal_easter()`),
/// for a year in `1..=max_year`.
///
/// Based on code by Simon Kershaw, <webmaster@ely.anglican.org>.
fn easter_days_after_march_21(year: i64, method: i64) -> i64 {
    // The Golden number.
    let golden = (year % 19) + 1;

    let julian = (year <= 1582 && method != CAL_EASTER_ALWAYS_GREGORIAN)
        || ((1583..=1752).contains(&year)
            && method != CAL_EASTER_ROMAN
            && method != CAL_EASTER_ALWAYS_GREGORIAN)
        || method == CAL_EASTER_ALWAYS_JULIAN;

    // The "Dominical number" (finding a Sunday) and the uncorrected date of
    // the Paschal full moon.
    let (mut dom, mut pfm);
    if julian {
        dom = (year + (year / 4) + 5) % 7;
        pfm = (3 - (11 * golden) - 7) % 30;
    } else {
        dom = (year + (year / 4) - (year / 100) + (year / 400)) % 7;

        // The solar and lunar corrections.
        let solar = (year - 1600) / 100 - (year - 1600) / 400;
        let lunar = (((year - 1400) / 100) * 8) / 25;

        pfm = (3 - (11 * golden) + solar - lunar) % 30;
    }
    if dom < 0 {
        dom += 7;
    }
    if pfm < 0 {
        pfm += 30;
    }

    // The corrected date of the Paschal full moon: days after 21st March.
    if pfm == 29 || (pfm == 28 && golden > 11) {
        pfm -= 1;
    }

    let mut tmp = (4 - pfm - dom) % 7;
    if tmp < 0 {
        tmp += 7;
    }

    pfm + tmp + 1
}

/// The timestamp of midnight on Easter (`gm` of `_cal_easter()`), in `tz`.
fn easter_timestamp(year: i64, method: i64, tz: &Arc<TzInfo>) -> i64 {
    let easter = easter_days_after_march_21(year, method);
    let (month, day) = if easter < 11 {
        (3, easter + 21)
    } else {
        (4, easter - 10)
    };
    mktime_midnight(tz, year, month, day)
}

/// `_cal_easter()`: with `gm` the timestamp at midnight on Easter of the given
/// year, otherwise the number of days Easter is after March 21st.
fn cal_easter(vm: &mut Vm, args: &Args, gm: bool) -> PResult<Value> {
    // (ZEND_LONG_MAX / 5) * 4
    const MAX_YEAR: i64 = (i64::MAX / 5) * 4;

    let method = args.int(1);
    let tz = libc_timezone();

    // Default to the current year if the year parameter is not given.
    let year = match args.int_opt(0) {
        Some(year) => year,
        None => localtime(&tz, now()).map_or(1900, |t| 1900 + i64::from(t.tm_year)),
    };

    if year <= 0 || year > MAX_YEAR {
        return Err(vm.argument_value_error(1, &format!("must be between 1 and {MAX_YEAR}")));
    }

    // 64-bit build: years between 1970 and 2.000.000.000.
    if gm && year < 1970 {
        // Timestamps only start after 1970.
        return Err(vm.argument_value_error(1, "must be a year after 1970 (inclusive)"));
    }

    if gm && year > 2_000_000_000 {
        // Timestamps only go up to the year 2.000.000.000.
        return Err(vm.argument_value_error(1, "must be a year before 2.000.000.000 (inclusive)"));
    }

    Ok(Value::Int(if gm {
        easter_timestamp(year, method, &tz)
    } else {
        easter_days_after_march_21(year, method)
    }))
}

/// Returns the timestamp of midnight on Easter of a given year (defaults to
/// the current year).
fn easter_date(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    cal_easter(vm, args, true)
}

/// Returns the number of days after March 21 that Easter falls on for a given
/// year (defaults to the current year).
fn easter_days(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    cal_easter(vm, args, false)
}

pub static EASTER_DATE: NativeFunction = NativeFunction {
    sig: "easter_date(?int $year = null, int $mode = CAL_EASTER_DEFAULT): int",
    handler: easter_date,
};

pub static EASTER_DAYS: NativeFunction = NativeFunction {
    sig: "easter_days(?int $year = null, int $mode = CAL_EASTER_DEFAULT): int",
    handler: easter_days,
};

#[cfg(test)]
pub(super) fn timestamp_in(year: i64, method: i64, tz: &Arc<TzInfo>) -> i64 {
    easter_timestamp(year, method, tz)
}
