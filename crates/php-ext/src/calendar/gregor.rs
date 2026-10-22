//! Port of `ext/calendar/gregor.c`: Gregorian calendar <-> serial day number.

use super::CalDate;

const GREGOR_SDN_OFFSET: i64 = 32045;
const DAYS_PER_5_MONTHS: i64 = 153;
const DAYS_PER_4_YEARS: i64 = 1461;
const DAYS_PER_400_YEARS: i64 = 146097;

/// `SdnToGregorian()`: the all-zero date for out-of-range input.
pub fn sdn_to_gregorian(sdn: i64) -> CalDate {
    convert(sdn).unwrap_or(CalDate::ZERO)
}

fn convert(sdn: i64) -> Option<CalDate> {
    if sdn <= 0 || sdn > (i64::MAX - 4 * GREGOR_SDN_OFFSET) / 4 {
        return None;
    }
    let mut temp = (sdn + GREGOR_SDN_OFFSET) * 4 - 1;

    if temp < 0 || temp / DAYS_PER_400_YEARS > i64::from(i32::MAX) {
        return None;
    }

    // The century (year / 100).
    let century = (temp / DAYS_PER_400_YEARS) as i32;

    // The year and the day of the year (1 <= day_of_year <= 366).
    temp = ((temp % DAYS_PER_400_YEARS) / 4) * 4 + 3;

    if i64::from(century) > i64::from(i32::MAX / 100) - (temp / DAYS_PER_4_YEARS) {
        return None;
    }

    let mut year = century * 100 + (temp / DAYS_PER_4_YEARS) as i32;
    let day_of_year = ((temp % DAYS_PER_4_YEARS) / 4 + 1) as i32;

    // The month and the day of the month.
    temp = i64::from(day_of_year) * 5 - 3;
    let mut month = (temp / DAYS_PER_5_MONTHS) as i32;
    let day = ((temp % DAYS_PER_5_MONTHS) / 5 + 1) as i32;

    // Convert to the normal beginning of the year.
    if month < 10 {
        month += 3;
    } else {
        year += 1;
        month -= 9;
    }

    // Adjust to the B.C./A.D. type numbering (there is no year 0).
    year -= 4800;
    if year <= 0 {
        year -= 1;
    }

    Some(CalDate { year, month, day })
}

/// `GregorianToSdn()`: 0 for invalid or out-of-range dates.
pub fn gregorian_to_sdn(input_year: i64, input_month: i64, input_day: i64) -> i64 {
    if input_year == 0
        || input_year < -4714
        || input_year > i64::from(i32::MAX) - 4800
        || input_month <= 0
        || input_month > 12
        || input_day <= 0
        || input_day > 31
    {
        return 0;
    }
    // Dates before SDN 1 (Nov 25, 4714 B.C.).
    if input_year == -4714 {
        if input_month < 11 {
            return 0;
        }
        if input_month == 11 && input_day < 25 {
            return 0;
        }
    }
    // Make the year always a positive number.
    let mut year = if input_year < 0 {
        input_year + 4801
    } else {
        input_year + 4800
    };

    // Adjust the start of the year.
    let month = if input_month > 2 {
        input_month - 3
    } else {
        year -= 1;
        input_month + 9
    };

    ((year / 100) * DAYS_PER_400_YEARS) / 4
        + ((year % 100) * DAYS_PER_4_YEARS) / 4
        + (month * DAYS_PER_5_MONTHS + 2) / 5
        + input_day
        - GREGOR_SDN_OFFSET
}

/// `MonthNameShort`.
pub const MONTH_NAME_SHORT: [&str; 13] = [
    "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `MonthNameLong`.
pub const MONTH_NAME_LONG: [&str; 13] = [
    "",
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
