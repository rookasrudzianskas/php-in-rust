//! Port of `ext/calendar/cal_unix.c`: Unix timestamps <-> Julian day numbers.

use php_runtime::{Args, NativeFunction, PResult, Value, Vm};

use super::gregor::gregorian_to_sdn;
use super::localtime::{libc_timezone, localtime, now};

const SECS_PER_DAY: i64 = 24 * 3600;

/// The Julian day of 1.1.1970.
const UNIX_EPOCH_JD: i64 = 2440588;

/// Converts a UNIX timestamp to a Julian Day.
fn unixtojd(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ts = match args.int_opt(0) {
        None => now(),
        Some(tl) if tl >= 0 => tl,
        Some(_) => {
            return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
        }
    };

    // `php_localtime_r()` is libc's `localtime_r()`: the zone comes from `TZ`.
    let Some(ta) = localtime(&libc_timezone(), ts) else {
        return Ok(Value::Bool(false));
    };

    // `ta->tm_year + 1900` is `int` arithmetic.
    Ok(Value::Int(gregorian_to_sdn(
        i64::from(ta.tm_year.wrapping_add(1900)),
        i64::from(ta.tm_mon + 1),
        i64::from(ta.tm_mday),
    )))
}

/// Converts a Julian Day to a UNIX timestamp.
fn jdtounix(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let uday = args.int(0);

    // Before the beginning of the Unix epoch or greater than representable.
    if uday < UNIX_EPOCH_JD || (uday - UNIX_EPOCH_JD) > (i64::MAX / SECS_PER_DAY) {
        return Err(vm.argument_value_error(
            1,
            &format!(
                "jday must be between {UNIX_EPOCH_JD} and {}",
                i64::MAX / SECS_PER_DAY + UNIX_EPOCH_JD
            ),
        ));
    }

    Ok(Value::Int((uday - UNIX_EPOCH_JD) * SECS_PER_DAY))
}

pub static UNIXTOJD: NativeFunction = NativeFunction {
    sig: "unixtojd(?int $timestamp = null): int|false",
    handler: unixtojd,
};

pub static JDTOUNIX: NativeFunction = NativeFunction {
    sig: "jdtounix(int $julian_day): int",
    handler: jdtounix,
};
