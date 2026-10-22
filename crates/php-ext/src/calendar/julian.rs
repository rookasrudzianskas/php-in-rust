//! Port of `ext/calendar/julian.c`: Julian calendar <-> serial day number.

use super::CalDate;

const JULIAN_SDN_OFFSET: i64 = 32083;
const DAYS_PER_5_MONTHS: i64 = 153;
const DAYS_PER_4_YEARS: i64 = 1461;

/// `SdnToJulian()`: the all-zero date for out-of-range input.
pub fn sdn_to_julian(sdn: i64) -> CalDate {
    convert(sdn).unwrap_or(CalDate::ZERO)
}

fn convert(sdn: i64) -> Option<CalDate> {
    if sdn <= 0 {
        return None;
    }
    // Check for overflow.
    if !(i64::MIN / 4..=(i64::MAX - JULIAN_SDN_OFFSET * 4 + 1) / 4).contains(&sdn) {
        return None;
    }
    let mut temp = sdn * 4 + (JULIAN_SDN_OFFSET * 4 - 1);

    // The year and the day of the year (1 <= day_of_year <= 366).
    let yearl = temp / DAYS_PER_4_YEARS;
    if yearl > i64::from(i32::MAX) || yearl < i64::from(i32::MIN) {
        return None;
    }
    let mut year = yearl as i32;
    let day_of_year = ((temp % DAYS_PER_4_YEARS) / 4 + 1) as i32;

    // The month and the day of the month.
    temp = i64::from(day_of_year) * 5 - 3;
    let mut month = (temp / DAYS_PER_5_MONTHS) as i32;
    let day = ((temp % DAYS_PER_5_MONTHS) / 5 + 1) as i32;

    // Convert to the normal beginning of the year. The C code does this in
    // `int` arithmetic, which wraps for the largest years.
    if month < 10 {
        month += 3;
    } else {
        year = year.wrapping_add(1);
        month -= 9;
    }

    // Adjust to the B.C./A.D. type numbering (there is no year 0).
    year = year.wrapping_sub(4800);
    if year <= 0 {
        year = year.wrapping_sub(1);
    }

    Some(CalDate { year, month, day })
}

/// `JulianToSdn()`: 0 for invalid or out-of-range dates.
pub fn julian_to_sdn(input_year: i64, input_month: i64, input_day: i64) -> i64 {
    if input_year == 0
        || input_year < -4713
        || input_year > i64::from(i32::MAX) - 4800
        || input_month <= 0
        || input_month > 12
        || input_day <= 0
        || input_day > 31
    {
        return 0;
    }
    // Dates before SDN 1 (Jan 2, 4713 B.C.).
    if input_year == -4713 && input_month == 1 && input_day == 1 {
        return 0;
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

    (year * DAYS_PER_4_YEARS) / 4 + (month * DAYS_PER_5_MONTHS + 2) / 5 + input_day
        - JULIAN_SDN_OFFSET
}
