//! Port of `ext/calendar/dow.c`: day of the week of a serial day number.

/// `DayOfWeek()`: 0 = Sunday, 1 = Monday, ..., 6 = Saturday.
pub fn day_of_week(sdn: i64) -> usize {
    (((sdn % 7 + 8) as i32) % 7) as usize
}

/// `DayNameShort`.
pub const DAY_NAME_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// `DayNameLong`.
pub const DAY_NAME_LONG: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
