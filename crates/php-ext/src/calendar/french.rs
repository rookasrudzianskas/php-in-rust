//! Port of `ext/calendar/french.c`: French Republican calendar <-> serial day number.

use super::CalDate;

const FRENCH_SDN_OFFSET: i64 = 2375474;
const DAYS_PER_4_YEARS: i64 = 1461;
const DAYS_PER_MONTH: i64 = 30;
const FIRST_VALID: i64 = 2375840;
const LAST_VALID: i64 = 2380952;

/// `SdnToFrench()`: the all-zero date outside the calendar's lifetime.
pub fn sdn_to_french(sdn: i64) -> CalDate {
    if !(FIRST_VALID..=LAST_VALID).contains(&sdn) {
        return CalDate::ZERO;
    }
    let temp = (sdn - FRENCH_SDN_OFFSET) * 4 - 1;
    let day_of_year = (temp % DAYS_PER_4_YEARS) / 4;
    CalDate {
        year: (temp / DAYS_PER_4_YEARS) as i32,
        month: (day_of_year / DAYS_PER_MONTH + 1) as i32,
        day: (day_of_year % DAYS_PER_MONTH + 1) as i32,
    }
}

/// `FrenchToSdn()`: 0 for invalid dates.
pub fn french_to_sdn(year: i64, month: i64, day: i64) -> i64 {
    if !(1..=14).contains(&year) || !(1..=13).contains(&month) || !(1..=30).contains(&day) {
        return 0;
    }
    (year * DAYS_PER_4_YEARS) / 4 + (month - 1) * DAYS_PER_MONTH + day + FRENCH_SDN_OFFSET
}

/// `FrenchMonthName`.
pub const FRENCH_MONTH_NAME: [&str; 14] = [
    "",
    "Vendemiaire",
    "Brumaire",
    "Frimaire",
    "Nivose",
    "Pluviose",
    "Ventose",
    "Germinal",
    "Floreal",
    "Prairial",
    "Messidor",
    "Thermidor",
    "Fructidor",
    "Extra",
];
