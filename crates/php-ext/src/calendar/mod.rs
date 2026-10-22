//! ext/calendar (port of `ext/calendar/calendar.c` and the SDN conversion
//! code of Scott E. Lee that it is built on).
//!
//! Layout follows php-src: one submodule per C file ([`dow`], [`easter`],
//! [`french`], [`gregor`], [`jewish`], [`julian`], [`cal_unix`]); this file
//! holds `calendar.c`.

mod cal_unix;
mod dow;
mod easter;
mod french;
mod gregor;
mod jewish;
mod julian;
mod localtime;

#[cfg(test)]
mod tests;

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, Registry, Value, Vm};

use dow::{DAY_NAME_LONG, DAY_NAME_SHORT, day_of_week};
use french::{FRENCH_MONTH_NAME, french_to_sdn, sdn_to_french};
use gregor::{MONTH_NAME_LONG, MONTH_NAME_SHORT, gregorian_to_sdn, sdn_to_gregorian};
use jewish::{JEWISH_MONTH_NAME_LEAP, jewish_to_sdn, sdn_to_jewish};
use julian::{julian_to_sdn, sdn_to_julian};

/// `ZEND_LONG_EXCEEDS_INT()` bounds.
const INT_MIN: i64 = i32::MIN as i64;
const INT_MAX: i64 = i32::MAX as i64;

// `enum cal_name_type_t`: the order must match `CAL_CONVERSION_TABLE`.
const CAL_GREGORIAN: i64 = 0;
const CAL_JULIAN: i64 = 1;
const CAL_JEWISH: i64 = 2;
const CAL_FRENCH: i64 = 3;
const CAL_NUM_CALS: i64 = 4;

// For jddayofweek().
const CAL_DOW_DAYNO: i64 = 0;
const CAL_DOW_LONG: i64 = 1;
const CAL_DOW_SHORT: i64 = 2;

// For jdmonthname().
const CAL_MONTH_GREGORIAN_SHORT: i64 = 0;
const CAL_MONTH_GREGORIAN_LONG: i64 = 1;
const CAL_MONTH_JULIAN_SHORT: i64 = 2;
const CAL_MONTH_JULIAN_LONG: i64 = 3;
const CAL_MONTH_JEWISH: i64 = 4;
const CAL_MONTH_FRENCH: i64 = 5;

// `php_calendar.h`: the easter calculation method.
const CAL_EASTER_DEFAULT: i64 = 0;
const CAL_EASTER_ROMAN: i64 = 1;
const CAL_EASTER_ALWAYS_GREGORIAN: i64 = 2;
const CAL_EASTER_ALWAYS_JULIAN: i64 = 3;

// jdtojewish() flags.
const CAL_JEWISH_ADD_ALAFIM_GERESH: i64 = 0x2;
const CAL_JEWISH_ADD_ALAFIM: i64 = 0x4;
const CAL_JEWISH_ADD_GERESHAYIM: i64 = 0x8;

/// A broken-down calendar date as produced by the `SdnTo*()` functions
/// (C `int`s; all zero when the day number is out of range).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CalDate {
    year: i32,
    month: i32,
    day: i32,
}

impl CalDate {
    const ZERO: CalDate = CalDate {
        year: 0,
        month: 0,
        day: 0,
    };

    /// `zend_strpprintf(0, "%i/%i/%i", month, day, year)`.
    fn to_php_string(self) -> String {
        format!("{}/{}/{}", self.month, self.day, self.year)
    }
}

/// `struct cal_entry_t`.
struct CalEntry {
    name: &'static str,
    symbol: &'static str,
    to_jd: fn(year: i64, month: i64, day: i64) -> i64,
    from_jd: fn(jd: i64) -> CalDate,
    num_months: usize,
    max_days_in_month: i64,
    month_name_short: &'static [&'static str],
    month_name_long: &'static [&'static str],
}

/// `cal_conversion_table`.
static CAL_CONVERSION_TABLE: [CalEntry; CAL_NUM_CALS as usize] = [
    CalEntry {
        name: "Gregorian",
        symbol: "CAL_GREGORIAN",
        to_jd: gregorian_to_sdn,
        from_jd: sdn_to_gregorian,
        num_months: 12,
        max_days_in_month: 31,
        month_name_short: &MONTH_NAME_SHORT,
        month_name_long: &MONTH_NAME_LONG,
    },
    CalEntry {
        name: "Julian",
        symbol: "CAL_JULIAN",
        to_jd: julian_to_sdn,
        from_jd: sdn_to_julian,
        num_months: 12,
        max_days_in_month: 31,
        month_name_short: &MONTH_NAME_SHORT,
        month_name_long: &MONTH_NAME_LONG,
    },
    CalEntry {
        name: "Jewish",
        symbol: "CAL_JEWISH",
        to_jd: jewish_to_sdn,
        from_jd: sdn_to_jewish,
        num_months: 13,
        max_days_in_month: 30,
        month_name_short: &JEWISH_MONTH_NAME_LEAP,
        month_name_long: &JEWISH_MONTH_NAME_LEAP,
    },
    CalEntry {
        name: "French",
        symbol: "CAL_FRENCH",
        to_jd: french_to_sdn,
        from_jd: sdn_to_french,
        num_months: 13,
        max_days_in_month: 30,
        month_name_short: &FRENCH_MONTH_NAME,
        month_name_long: &FRENCH_MONTH_NAME,
    },
];

/// The `cal < 0 || cal >= CAL_NUM_CALS` check shared by the functions taking a
/// calendar ID; `arg_num` is the position of the `$calendar` parameter.
fn calendar_arg(vm: &mut Vm, cal: i64, arg_num: usize) -> PResult<&'static CalEntry> {
    usize::try_from(cal)
        .ok()
        .and_then(|i| CAL_CONVERSION_TABLE.get(i))
        .ok_or_else(|| vm.argument_value_error(arg_num, "must be a valid calendar ID"))
}

/// Month name lookup that mirrors the C array indexing for the month numbers
/// the conversion functions can produce.
fn name_at(names: &[&'static str], index: i32) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i).copied())
        .unwrap_or("")
}

fn int_to_index(n: i32) -> usize {
    usize::try_from(n).unwrap_or(0)
}

/// `JEWISH_MONTH_NAME(year)[month]` for positive years, "" otherwise.
fn jewish_month_name(year: i32, month: i32) -> &'static str {
    if year > 0 {
        name_at(jewish::month_names(year), month)
    } else {
        ""
    }
}

// ---------------------------------------------------------------------------
// cal_info
// ---------------------------------------------------------------------------

/// `_php_cal_info()`.
fn php_cal_info(calendar: &CalEntry) -> Value {
    let mut months = PhpArray::new();
    let mut smonths = PhpArray::new();

    for i in 1..=calendar.num_months {
        let key = ArrayKey::Int(i as i64);
        months.insert(key.clone(), Value::str(calendar.month_name_long[i]));
        smonths.insert(key, Value::str(calendar.month_name_short[i]));
    }

    let mut ret = PhpArray::new();
    ret.insert_str("months", Value::array(months));
    ret.insert_str("abbrevmonths", Value::array(smonths));
    ret.insert_str("maxdaysinmonth", calendar.max_days_in_month);
    ret.insert_str("calname", calendar.name);
    ret.insert_str("calsymbol", calendar.symbol);
    Value::array(ret)
}

/// Returns information about a particular calendar.
fn cal_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cal = args.int(0);

    if cal == -1 {
        let mut ret = PhpArray::with_capacity(CAL_NUM_CALS as usize);
        for (i, calendar) in CAL_CONVERSION_TABLE.iter().enumerate() {
            ret.insert(ArrayKey::Int(i as i64), php_cal_info(calendar));
        }
        return Ok(Value::array(ret));
    }

    let calendar = calendar_arg(vm, cal, 1)?;
    Ok(php_cal_info(calendar))
}

/// The month/day/year range checks shared by `cal_days_in_month()` and
/// `cal_to_jd()`.
fn check_month(vm: &mut Vm, month: i64) -> PResult<()> {
    if month <= 0 || month > INT_MAX - 1 {
        return Err(vm.argument_value_error(2, &format!("must be between 1 and {}", INT_MAX - 1)));
    }
    Ok(())
}

fn check_year(vm: &mut Vm, year: i64, arg_num: usize) -> PResult<()> {
    if year > INT_MAX - 1 {
        return Err(vm.argument_value_error(arg_num, &format!("must be less than {}", INT_MAX - 1)));
    }
    Ok(())
}

/// Returns the number of days in a month for a given year and calendar.
fn cal_days_in_month(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (cal, month, year) = (args.int(0), args.int(1), args.int(2));

    let calendar = calendar_arg(vm, cal, 1)?;
    check_month(vm, month)?;
    check_year(vm, year, 3)?;

    let sdn_start = (calendar.to_jd)(year, month, 1);

    if sdn_start == 0 {
        return Err(vm.throw_value_error("Invalid date"));
    }

    let mut sdn_next = (calendar.to_jd)(year, 1 + month, 1);

    if sdn_next == 0 {
        // If the next month is invalid, then we need to try the first month of
        // the next year, bearing in mind that the next year after 1 BCE is
        // actually 1 AD and not 0.
        if year == -1 {
            sdn_next = (calendar.to_jd)(1, 1, 1);
        } else {
            sdn_next = (calendar.to_jd)(year + 1, 1, 1);
            if cal == CAL_FRENCH && sdn_next == 0 {
                // The French calendar ends on 0014-13-05.
                sdn_next = 2380953;
            }
        }
    }

    Ok(Value::Int(sdn_next - sdn_start))
}

/// Converts from a supported calendar to Julian Day Count.
fn cal_to_jd(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (cal, month, day, year) = (args.int(0), args.int(1), args.int(2), args.int(3));

    let calendar = calendar_arg(vm, cal, 1)?;
    check_month(vm, month)?;

    if !(INT_MIN..=INT_MAX).contains(&day) {
        return Err(vm.argument_value_error(3, &format!("must be between {INT_MIN} and {INT_MAX}")));
    }

    check_year(vm, year, 4)?;

    Ok(Value::Int((calendar.to_jd)(year, month, day)))
}

/// Converts from Julian Day Count to a supported calendar and returns
/// extended information.
fn cal_from_jd(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (jd, cal) = (args.int(0), args.int(1));

    let calendar = calendar_arg(vm, cal, 2)?;
    let date = (calendar.from_jd)(jd);
    let CalDate { year, month, day } = date;

    let mut ret = PhpArray::new();
    ret.insert_str("date", date.to_php_string());
    ret.insert_str("month", month);
    ret.insert_str("day", day);
    ret.insert_str("year", year);

    // Day of week.
    if cal != CAL_JEWISH || year > 0 {
        let dow = day_of_week(jd);
        ret.insert_str("dow", dow as i64);
        ret.insert_str("abbrevdayname", DAY_NAME_SHORT[dow]);
        ret.insert_str("dayname", DAY_NAME_LONG[dow]);
    } else {
        ret.insert_str("dow", Value::Null);
        ret.insert_str("abbrevdayname", "");
        ret.insert_str("dayname", "");
    }

    // Month name.
    if cal == CAL_JEWISH {
        // Special case for the Jewish calendar.
        let name = jewish_month_name(year, month);
        ret.insert_str("abbrevmonth", name);
        ret.insert_str("monthname", name);
    } else {
        ret.insert_str("abbrevmonth", name_at(calendar.month_name_short, month));
        ret.insert_str("monthname", name_at(calendar.month_name_long, month));
    }
    Ok(Value::array(ret))
}

/// Converts a julian day count to a gregorian calendar date.
fn jdtogregorian(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::from(sdn_to_gregorian(args.int(0)).to_php_string()))
}

/// Converts a gregorian calendar date to julian day count.
fn gregoriantojd(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (month, day, year) = (args.int(0), args.int(1), args.int(2));
    Ok(Value::Int(gregorian_to_sdn(year, month, day)))
}

/// Converts a julian day count to a julian calendar date.
fn jdtojulian(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::from(sdn_to_julian(args.int(0)).to_php_string()))
}

/// Converts a julian calendar date to julian day count.
fn juliantojd(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (month, day, year) = (args.int(0), args.int(1), args.int(2));
    Ok(Value::Int(julian_to_sdn(year, month, day)))
}

/// For `heb_number_to_chars` escape sequences of the ISO-8859-8 Hebrew
/// alphabet.
const ALEF_BET: [u8; 23] = [
    b'0', 0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEB, 0xEC, 0xEE, 0xF0, 0xF1,
    0xF2, 0xF4, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA,
];

/// `heb_number_to_chars()`: a number as Hebrew letters (ISO-8859-8). The
/// format is not unique: both year 5 and year 5000 give the letter he.
/// `None` outside 1..=9999.
fn heb_number_to_chars(mut n: i32, fl: i32) -> Option<Vec<u8>> {
    if !(1..=9999).contains(&n) {
        return None;
    }
    let mut out: Vec<u8> = Vec::with_capacity(18);
    let mut end_of_alafim = 0;

    // Alafim (thousands).
    if n / 1000 != 0 {
        out.push(ALEF_BET[(n / 1000) as usize]);

        if fl & CAL_JEWISH_ADD_ALAFIM_GERESH as i32 != 0 {
            out.push(b'\'');
        }
        if fl & CAL_JEWISH_ADD_ALAFIM as i32 != 0 {
            // ISO-8859-8 for the word "alafim" surrounded by spaces.
            out.extend_from_slice(b" \xE0\xEC\xF4\xE9\xED ");
        }

        end_of_alafim = out.len();
        n %= 1000;
    }

    // Tav-tav (tav = 400).
    while n >= 400 {
        out.push(ALEF_BET[22]);
        n -= 400;
    }

    // Meot (hundreds).
    if n >= 100 {
        out.push(ALEF_BET[18 + (n / 100) as usize]);
        n %= 100;
    }

    if n == 15 || n == 16 {
        // Tet-vav and tet-zain (special case for 15 and 16).
        out.push(ALEF_BET[9]);
        out.push(ALEF_BET[(n - 9) as usize]);
    } else {
        // Asarot (tens).
        if n >= 10 {
            out.push(ALEF_BET[9 + (n / 10) as usize]);
            n %= 10;
        }
        // Yehidot (ones).
        if n > 0 {
            out.push(ALEF_BET[n as usize]);
        }
    }

    if fl & CAL_JEWISH_ADD_GERESHAYIM as i32 != 0 {
        match out.len() - end_of_alafim {
            0 => {}
            1 => out.push(b'\''),
            _ => {
                let last = out.pop().unwrap_or(b'\'');
                out.push(b'"');
                out.push(last);
            }
        }
    }
    Some(out)
}

/// Converts a julian day count to a jewish calendar date.
fn jdtojewish(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let julday = args.int(0);
    let heb = args.bool(1);
    let fl = args.int(2);

    let CalDate { year, month, day } = sdn_to_jewish(julday);
    if !heb {
        return Ok(Value::from(CalDate { year, month, day }.to_php_string()));
    }

    if year <= 0 || year > 9999 {
        return Err(vm.throw_value_error("Year out of range (0-9999)"));
    }

    // The flags are an `int` in heb_number_to_chars().
    let fl = fl as i32;
    // `%s` of a NULL pointer prints "(null)"; unreachable for valid dates.
    let letters = |n: i32| heb_number_to_chars(n, fl).unwrap_or_else(|| b"(null)".to_vec());
    let mut out = letters(day);
    out.push(b' ');
    out.extend_from_slice(
        jewish::heb_month_names(year)
            .get(int_to_index(month))
            .copied()
            .unwrap_or(b""),
    );
    out.push(b' ');
    out.extend_from_slice(&letters(year));
    Ok(Value::bytes(&out))
}

/// Converts a jewish calendar date to a julian day count.
fn jewishtojd(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (month, day, year) = (args.int(0), args.int(1), args.int(2));

    if !(INT_MIN..=INT_MAX).contains(&year) {
        return Err(vm.argument_value_error(3, &format!("must be between {INT_MIN} and {INT_MAX}")));
    }

    Ok(Value::Int(jewish_to_sdn(year, month, day)))
}

/// Converts a julian day count to a french republic calendar date.
fn jdtofrench(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::from(sdn_to_french(args.int(0)).to_php_string()))
}

/// Converts a french republic calendar date to julian day count.
fn frenchtojd(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (month, day, year) = (args.int(0), args.int(1), args.int(2));
    Ok(Value::Int(french_to_sdn(year, month, day)))
}

/// Returns name or number of day of week from julian day count.
fn jddayofweek(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let julday = args.int(0);
    let mode = args.int(1);

    let day = day_of_week(julday);

    Ok(match mode {
        CAL_DOW_LONG => Value::str(DAY_NAME_LONG[day]),
        CAL_DOW_SHORT => Value::str(DAY_NAME_SHORT[day]),
        // CAL_DOW_DAYNO and anything else.
        _ => Value::Int(day as i64),
    })
}

/// Returns name of month for julian day count.
fn jdmonthname(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let julday = args.int(0);
    let mode = args.int(1);

    let monthname = match mode {
        CAL_MONTH_GREGORIAN_LONG => name_at(&MONTH_NAME_LONG, sdn_to_gregorian(julday).month),
        CAL_MONTH_JULIAN_SHORT => name_at(&MONTH_NAME_SHORT, sdn_to_julian(julday).month),
        CAL_MONTH_JULIAN_LONG => name_at(&MONTH_NAME_LONG, sdn_to_julian(julday).month),
        CAL_MONTH_JEWISH => {
            let CalDate { year, month, .. } = sdn_to_jewish(julday);
            jewish_month_name(year, month)
        }
        CAL_MONTH_FRENCH => name_at(&FRENCH_MONTH_NAME, sdn_to_french(julday).month),
        // Default Gregorian (CAL_MONTH_GREGORIAN_SHORT).
        _ => name_at(&MONTH_NAME_SHORT, sdn_to_gregorian(julday).month),
    };

    Ok(Value::str(monthname))
}

macro_rules! native {
    ($static_name:ident, $handler:ident, $sig:literal) => {
        static $static_name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

native!(
    CAL_DAYS_IN_MONTH,
    cal_days_in_month,
    "cal_days_in_month(int $calendar, int $month, int $year): int"
);
native!(
    CAL_FROM_JD,
    cal_from_jd,
    "cal_from_jd(int $julian_day, int $calendar): array"
);
native!(CAL_INFO, cal_info, "cal_info(int $calendar = -1): array");
native!(
    CAL_TO_JD,
    cal_to_jd,
    "cal_to_jd(int $calendar, int $month, int $day, int $year): int"
);
native!(
    FRENCHTOJD,
    frenchtojd,
    "frenchtojd(int $month, int $day, int $year): int"
);
native!(
    GREGORIANTOJD,
    gregoriantojd,
    "gregoriantojd(int $month, int $day, int $year): int"
);
native!(
    JDDAYOFWEEK,
    jddayofweek,
    "jddayofweek(int $julian_day, int $mode = CAL_DOW_DAYNO): int|string"
);
native!(
    JDMONTHNAME,
    jdmonthname,
    "jdmonthname(int $julian_day, int $mode): string"
);
native!(
    JDTOFRENCH,
    jdtofrench,
    "jdtofrench(int $julian_day): string"
);
native!(
    JDTOGREGORIAN,
    jdtogregorian,
    "jdtogregorian(int $julian_day): string"
);
native!(
    JDTOJEWISH,
    jdtojewish,
    "jdtojewish(int $julian_day, bool $hebrew = false, int $flags = 0): string"
);
native!(
    JDTOJULIAN,
    jdtojulian,
    "jdtojulian(int $julian_day): string"
);
native!(
    JEWISHTOJD,
    jewishtojd,
    "jewishtojd(int $month, int $day, int $year): int"
);
native!(
    JULIANTOJD,
    juliantojd,
    "juliantojd(int $month, int $day, int $year): int"
);

/// `PHP_MINIT(calendar)` / `register_calendar_symbols()`.
pub fn register(r: &mut Registry) {
    r.functions(&[
        &CAL_DAYS_IN_MONTH,
        &CAL_FROM_JD,
        &CAL_INFO,
        &CAL_TO_JD,
        &easter::EASTER_DATE,
        &easter::EASTER_DAYS,
        &FRENCHTOJD,
        &GREGORIANTOJD,
        &JDDAYOFWEEK,
        &JDMONTHNAME,
        &JDTOFRENCH,
        &JDTOGREGORIAN,
        &JDTOJEWISH,
        &JDTOJULIAN,
        &cal_unix::JDTOUNIX,
        &JEWISHTOJD,
        &JULIANTOJD,
        &cal_unix::UNIXTOJD,
    ]);

    for (name, value) in [
        ("CAL_GREGORIAN", CAL_GREGORIAN),
        ("CAL_JULIAN", CAL_JULIAN),
        ("CAL_JEWISH", CAL_JEWISH),
        ("CAL_FRENCH", CAL_FRENCH),
        ("CAL_NUM_CALS", CAL_NUM_CALS),
        ("CAL_DOW_DAYNO", CAL_DOW_DAYNO),
        ("CAL_DOW_SHORT", CAL_DOW_SHORT),
        ("CAL_DOW_LONG", CAL_DOW_LONG),
        ("CAL_MONTH_GREGORIAN_SHORT", CAL_MONTH_GREGORIAN_SHORT),
        ("CAL_MONTH_GREGORIAN_LONG", CAL_MONTH_GREGORIAN_LONG),
        ("CAL_MONTH_JULIAN_SHORT", CAL_MONTH_JULIAN_SHORT),
        ("CAL_MONTH_JULIAN_LONG", CAL_MONTH_JULIAN_LONG),
        ("CAL_MONTH_JEWISH", CAL_MONTH_JEWISH),
        ("CAL_MONTH_FRENCH", CAL_MONTH_FRENCH),
        ("CAL_EASTER_DEFAULT", CAL_EASTER_DEFAULT),
        ("CAL_EASTER_ROMAN", CAL_EASTER_ROMAN),
        ("CAL_EASTER_ALWAYS_GREGORIAN", CAL_EASTER_ALWAYS_GREGORIAN),
        ("CAL_EASTER_ALWAYS_JULIAN", CAL_EASTER_ALWAYS_JULIAN),
        ("CAL_JEWISH_ADD_ALAFIM_GERESH", CAL_JEWISH_ADD_ALAFIM_GERESH),
        ("CAL_JEWISH_ADD_ALAFIM", CAL_JEWISH_ADD_ALAFIM),
        ("CAL_JEWISH_ADD_GERESHAYIM", CAL_JEWISH_ADD_GERESHAYIM),
    ] {
        r.constant(name, value);
    }
}
