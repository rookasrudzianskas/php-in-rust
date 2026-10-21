//! Calendar helpers (`dow.c`): day of week, day of year, ISO weeks, validity checks.

const M_TABLE_COMMON: [i64; 13] = [-1, 0, 3, 3, 6, 1, 4, 6, 2, 5, 0, 3, 5];
const M_TABLE_LEAP: [i64; 13] = [-1, 6, 2, 3, 6, 1, 4, 6, 2, 5, 0, 3, 5];

const D_TABLE_COMMON: [i64; 13] = [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
const D_TABLE_LEAP: [i64; 13] = [0, 0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
const ML_TABLE_COMMON: [i64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const ML_TABLE_LEAP: [i64; 13] = [0, 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// Gregorian leap year test (`timelib_is_leap`).
pub fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn positive_mod(x: i64, y: i64) -> i64 {
    let tmp = x % y;
    if tmp < 0 { tmp + y } else { tmp }
}

fn century_value(j: i64) -> i64 {
    6 - positive_mod(j, 4) * 2
}

/// Table lookups with an out-of-range month behave like reading garbage in C; we clamp
/// to the table so that we never panic.
fn table(t: &[i64; 13], m: i64) -> i64 {
    t[m.clamp(0, 12) as usize]
}

fn day_of_week_ex(y: i64, m: i64, d: i64, iso: bool) -> i64 {
    let c1 = century_value(positive_mod(y, 400) / 100);
    let y1 = positive_mod(y, 100);
    let m1 = if is_leap(y) {
        table(&M_TABLE_LEAP, m)
    } else {
        table(&M_TABLE_COMMON, m)
    };
    let mut dow = positive_mod(
        c1.wrapping_add(y1)
            .wrapping_add(m1)
            .wrapping_add(y1 / 4)
            .wrapping_add(d),
        7,
    );
    if iso && dow == 0 {
        dow = 7;
    }
    dow
}

/// Day of week, 0 = Sunday .. 6 = Saturday (`timelib_day_of_week`).
pub fn day_of_week(y: i64, m: i64, d: i64) -> i64 {
    day_of_week_ex(y, m, d, false)
}

/// ISO day of week, 1 = Monday .. 7 = Sunday (`timelib_iso_day_of_week`).
pub fn iso_day_of_week(y: i64, m: i64, d: i64) -> i64 {
    day_of_week_ex(y, m, d, true)
}

/// Day of year, 0 = Jan 1st (`timelib_day_of_year`).
pub fn day_of_year(y: i64, m: i64, d: i64) -> i64 {
    let base = if is_leap(y) {
        table(&D_TABLE_LEAP, m)
    } else {
        table(&D_TABLE_COMMON, m)
    };
    base + d - 1
}

/// Number of days in month `m` of year `y` (`timelib_days_in_month`).
pub fn days_in_month(y: i64, m: i64) -> i64 {
    if is_leap(y) {
        table(&ML_TABLE_LEAP, m)
    } else {
        table(&ML_TABLE_COMMON, m)
    }
}

/// ISO week number and ISO year, returned as `(iso_week, iso_year)`
/// (`timelib_isoweek_from_date`).
pub fn isoweek_from_date(y: i64, m: i64, d: i64) -> (i64, i64) {
    let y_leap = is_leap(y);
    let prev_y_leap = is_leap(y - 1);
    let mut doy = day_of_year(y, m, d) + 1;
    if y_leap && m > 2 {
        doy += 1;
    }
    let mut jan1weekday = day_of_week(y, 1, 1);
    let mut weekday = day_of_week(y, m, d);
    if weekday == 0 {
        weekday = 7;
    }
    if jan1weekday == 0 {
        jan1weekday = 7;
    }
    let mut iw = 0;
    let iy;
    // Find if Y M D falls in YearNumber Y-1, WeekNumber 52 or 53
    if doy <= (8 - jan1weekday) && jan1weekday > 4 {
        iy = y - 1;
        iw = if jan1weekday == 5 || (jan1weekday == 6 && prev_y_leap) {
            53
        } else {
            52
        };
    } else {
        iy = y;
    }
    // Find if Y M D falls in YearNumber Y+1, WeekNumber 1
    if iy == y {
        let i = if y_leap { 366 } else { 365 };
        if (i - (doy - i64::from(y_leap))) < (4 - weekday) {
            return (1, y + 1);
        }
    }
    // Find if Y M D falls in YearNumber Y, WeekNumber 1 through 53
    if iy == y {
        let j = doy + (7 - weekday) + (jan1weekday - 1);
        iw = j / 7;
        if jan1weekday > 4 {
            iw -= 1;
        }
    }
    (iw, iy)
}

/// ISO year, week and day of week, returned as `(iy, iw, id)` (`timelib_isodate_from_date`).
pub fn isodate_from_date(y: i64, m: i64, d: i64) -> (i64, i64, i64) {
    let (iw, iy) = isoweek_from_date(y, m, d);
    (iy, iw, day_of_week_ex(y, m, d, true))
}

/// Day of year (0-based, may fall outside the year) for an ISO year/week/day
/// (`timelib_daynr_from_weeknr`).
pub fn daynr_from_weeknr(iy: i64, iw: i64, id: i64) -> i64 {
    // Figure out the dayofweek for y-1-1
    let dow = day_of_week(iy, 1, 1);
    // then use that to figure out the offset for day 1 of week 1
    let day = 0 - if dow > 4 { dow - 7 } else { dow };
    // Add weeks and days
    day + (iw - 1) * 7 + id
}

/// Calendar date `(y, m, d)` for an ISO year/week/day (`timelib_date_from_isodate`).
pub fn date_from_isodate(iy: i64, iw: i64, id: i64) -> (i64, i64, i64) {
    let mut daynr = daynr_from_weeknr(iy, iw, id) + 1;
    let mut y = iy;
    let mut leap = is_leap(y);

    while daynr <= 0 {
        y -= 1;
        leap = is_leap(y);
        daynr += if leap { 366 } else { 365 };
    }
    while daynr > if leap { 366 } else { 365 } {
        daynr -= if leap { 366 } else { 365 };
        y += 1;
        leap = is_leap(y);
    }
    let t = if leap {
        &ML_TABLE_LEAP
    } else {
        &ML_TABLE_COMMON
    };
    let mut m = 1;
    while m < 12 && daynr > t[m as usize] {
        daynr -= t[m as usize];
        m += 1;
    }
    (y, m, daynr)
}

/// Whether h:i:s lies within 00:00:00..23:59:59 (`timelib_valid_time`).
pub fn valid_time(h: i64, i: i64, s: i64) -> bool {
    !(!(0..=23).contains(&h) || !(0..=59).contains(&i) || !(0..=59).contains(&s))
}

/// Whether y-m-d is a valid Gregorian date (`timelib_valid_date`).
pub fn valid_date(y: i64, m: i64, d: i64) -> bool {
    !(!(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dow_basics() {
        assert_eq!(day_of_week(1978, 12, 22), 5);
        assert_eq!(day_of_week(2005, 2, 19), 6);
        assert_eq!(iso_day_of_week(2023, 1, 1), 7);
        assert_eq!(day_of_week(-1, 1, 1), day_of_week(399, 1, 1));
    }

    #[test]
    fn iso_weeks() {
        assert_eq!(isoweek_from_date(2008, 12, 29), (1, 2009));
        assert_eq!(isoweek_from_date(2010, 1, 3), (53, 2009));
        assert_eq!(isoweek_from_date(2021, 1, 1), (53, 2020));
        assert_eq!(isoweek_from_date(2024, 12, 30), (1, 2025));
        assert_eq!(isoweek_from_date(2023, 6, 15), (24, 2023));
        assert_eq!(date_from_isodate(2009, 1, 1), (2008, 12, 29));
        assert_eq!(date_from_isodate(2020, 53, 5), (2021, 1, 1));
    }

    #[test]
    fn year_helpers() {
        assert!(is_leap(2000));
        assert!(!is_leap(1900));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(day_of_year(2024, 3, 1), 60);
        assert!(valid_date(2024, 2, 29));
        assert!(!valid_date(2023, 2, 29));
        assert!(valid_time(23, 59, 59));
        assert!(!valid_time(24, 0, 0));
    }
}
