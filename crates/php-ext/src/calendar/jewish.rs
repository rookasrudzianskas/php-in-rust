//! Port of `ext/calendar/jewish.c`: Jewish calendar <-> serial day number.
//!
//! The C code mixes `int` and `zend_long` variables; where it stores a
//! `zend_long` into an `int` (the `tishri1` values) the port truncates the
//! same way.

use super::CalDate;

const HALAKIM_PER_HOUR: i64 = 1080;
const HALAKIM_PER_DAY: i64 = 25920;
const HALAKIM_PER_LUNAR_CYCLE: i64 = 29 * HALAKIM_PER_DAY + 13753;
const HALAKIM_PER_METONIC_CYCLE: i64 = HALAKIM_PER_LUNAR_CYCLE * (12 * 19 + 7);

const JEWISH_SDN_OFFSET: i64 = 347997;
/// 12/13/887605; a greater value raises an integer overflow.
const JEWISH_SDN_MAX: i64 = 324542846;
const NEW_MOON_OF_CREATION: i64 = 31524;

const SUNDAY: i64 = 0;
const MONDAY: i64 = 1;
const TUESDAY: i64 = 2;
const WEDNESDAY: i64 = 3;
const FRIDAY: i64 = 5;

const NOON: i64 = 18 * HALAKIM_PER_HOUR;
const AM3_11_20: i64 = 9 * HALAKIM_PER_HOUR + 204;
const AM9_32_43: i64 = 15 * HALAKIM_PER_HOUR + 589;

/// `monthsPerYear`: months in each year of the 19 year metonic cycle.
pub const MONTHS_PER_YEAR: [i64; 19] = [
    12, 12, 13, 12, 12, 13, 12, 13, 12, 12, 13, 12, 12, 13, 12, 12, 13, 12, 13,
];

const YEAR_OFFSET: [i64; 19] = [
    0, 12, 24, 37, 49, 61, 74, 86, 99, 111, 123, 136, 148, 160, 173, 185, 197, 210, 222,
];

/// Names for a leap (13 month) year.
pub const JEWISH_MONTH_NAME_LEAP: [&str; 14] = [
    "", "Tishri", "Heshvan", "Kislev", "Tevet", "Shevat", "Adar I", "Adar II", "Nisan", "Iyyar",
    "Sivan", "Tammuz", "Av", "Elul",
];

/// Names for a regular year.
pub const JEWISH_MONTH_NAME: [&str; 14] = [
    "", "Tishri", "Heshvan", "Kislev", "Tevet", "Shevat", "", "Adar", "Nisan", "Iyyar", "Sivan",
    "Tammuz", "Av", "Elul",
];

/// Hebrew (ISO-8859-8) names for a leap year.
pub const JEWISH_MONTH_HEB_NAME_LEAP: [&[u8]; 14] = [
    b"",
    b"\xFA\xF9\xF8\xE9",
    b"\xE7\xF9\xE5\xEF",
    b"\xEB\xF1\xEC\xE5",
    b"\xE8\xE1\xFA",
    b"\xF9\xE1\xE8",
    b"\xE0\xE3\xF8 \xE0'",
    b"\xE0\xE3\xF8 \xE1'",
    b"\xF0\xE9\xF1\xEF",
    b"\xE0\xE9\xE9\xF8",
    b"\xF1\xE9\xe5\xEF",
    b"\xFA\xEE\xE5\xE6",
    b"\xE0\xE1",
    b"\xE0\xEC\xE5\xEC",
];

/// Hebrew (ISO-8859-8) names for a regular year.
pub const JEWISH_MONTH_HEB_NAME: [&[u8]; 14] = [
    b"",
    b"\xFA\xF9\xF8\xE9",
    b"\xE7\xF9\xE5\xEF",
    b"\xEB\xF1\xEC\xE5",
    b"\xE8\xE1\xFA",
    b"\xF9\xE1\xE8",
    b"",
    b"\xE0\xE3\xF8",
    b"\xF0\xE9\xF1\xEF",
    b"\xE0\xE9\xE9\xF8",
    b"\xF1\xE9\xE5\xEF",
    b"\xFA\xEE\xE5\xE6",
    b"\xE0\xE1",
    b"\xE0\xEC\xE5\xEC",
];

/// `monthsPerYear[index]` for the (non-negative) metonic index `(year - 1) % 19`.
fn months_per_year(index: i32) -> i64 {
    usize::try_from(index)
        .ok()
        .and_then(|i| MONTHS_PER_YEAR.get(i).copied())
        .unwrap_or(12)
}

fn is_leap(year: i32) -> bool {
    months_per_year((year - 1) % 19) == 13
}

/// `JEWISH_MONTH_NAME(year)`; only meaningful for `year > 0`.
pub fn month_names(year: i32) -> &'static [&'static str; 14] {
    if is_leap(year) {
        &JEWISH_MONTH_NAME_LEAP
    } else {
        &JEWISH_MONTH_NAME
    }
}

/// `JEWISH_HEB_MONTH_NAME(year)`; only meaningful for `year > 0`.
pub fn heb_month_names(year: i32) -> &'static [&'static [u8]; 14] {
    if is_leap(year) {
        &JEWISH_MONTH_HEB_NAME_LEAP
    } else {
        &JEWISH_MONTH_HEB_NAME
    }
}

/// `Tishri1()`: given the year within the metonic cycle and the molad that
/// starts it, the day on which the year really starts (after the four
/// dehiyyot postponement rules).
fn tishri_1(metonic_year: i32, molad_day: i64, molad_halakim: i64) -> i64 {
    let mut tishri1 = molad_day;
    let mut dow = tishri1 % 7;
    let leap_year = matches!(metonic_year, 2 | 5 | 7 | 10 | 13 | 16 | 18);
    let last_was_leap_year = matches!(metonic_year, 3 | 6 | 8 | 11 | 14 | 17 | 0);

    // Apply rules 2, 3 and 4.
    if molad_halakim >= NOON
        || (!leap_year && dow == TUESDAY && molad_halakim >= AM3_11_20)
        || (last_was_leap_year && dow == MONDAY && molad_halakim >= AM9_32_43)
    {
        tishri1 += 1;
        dow += 1;
        if dow == 7 {
            dow = 0;
        }
    }
    // Apply rule 1 after the others because it can cause an additional
    // delay of one day.
    if dow == WEDNESDAY || dow == FRIDAY || dow == SUNDAY {
        tishri1 += 1;
    }
    tishri1
}

/// `MoladOfMetonicCycle()`: day and halakim of the molad that starts a
/// metonic cycle, computed with the C code's 16-bit limb arithmetic and its
/// overflow guards (`(0, 0)` when it would overflow).
fn molad_of_metonic_cycle(metonic_cycle: i32) -> (i64, i64) {
    const LOW: i64 = HALAKIM_PER_METONIC_CYCLE & 0xFFFF;
    const HIGH: i64 = (HALAKIM_PER_METONIC_CYCLE >> 16) & 0xFFFF;

    // Start with the time of the first molad after creation.
    let mut r1 = NEW_MOON_OF_CREATION as u64;
    let chk = i64::from(metonic_cycle);

    if chk > (i64::MAX - NEW_MOON_OF_CREATION) / LOW {
        return (0, 0);
    }

    // metonic_cycle * HALAKIM_PER_METONIC_CYCLE: the upper 32 bits of the
    // result end up in r2 and the lower 16 bits in r1.
    r1 = r1.wrapping_add((chk * LOW) as u64);

    // The C comparison converts `chk` to unsigned, so a negative cycle fails.
    if (chk as u64) > ((i64::MAX as u64).wrapping_sub(r1 >> 16)) / HIGH as u64 {
        return (0, 0);
    }

    let mut r2 = r1 >> 16;
    r2 = r2.wrapping_add((chk * HIGH) as u64);

    // r2r1 / HALAKIM_PER_DAY: the remainder ends up in r1, the upper 16 bits
    // of the quotient in d2 and the lower 16 bits in d1.
    let per_day = HALAKIM_PER_DAY as u64;
    let d2 = r2 / per_day;
    r2 -= d2 * per_day;
    r1 = (r2 << 16) | (r1 & 0xFFFF);
    let d1 = r1 / per_day;
    r1 -= d1 * per_day;

    (((d2 << 16) | d1) as i64, r1 as i64)
}

/// Result of [`find_tishri_molad`].
struct TishriMolad {
    metonic_cycle: i32,
    metonic_year: i32,
    molad_day: i64,
    molad_halakim: i64,
}

/// `FindTishriMolad()`: the molad of Tishri closest to `input_day`.
fn find_tishri_molad(input_day: i64) -> TishriMolad {
    // Estimate the metonic cycle number. It may be an under estimate (there
    // are 6939.6896 days in a cycle, not 6940) but never an over estimate.
    let mut metonic_cycle = ((input_day + 310) / 6940) as i32;

    let (mut molad_day, mut molad_halakim) = molad_of_metonic_cycle(metonic_cycle);

    // Correct an under estimate.
    while molad_day < input_day - 6940 + 310 {
        metonic_cycle += 1;
        molad_halakim += HALAKIM_PER_METONIC_CYCLE;
        molad_day += molad_halakim / HALAKIM_PER_DAY;
        molad_halakim %= HALAKIM_PER_DAY;
    }

    // Find the molad of Tishri closest to this date.
    let mut metonic_year = 0;
    while metonic_year < 18 {
        if molad_day > input_day - 74 {
            break;
        }
        molad_halakim += HALAKIM_PER_LUNAR_CYCLE * MONTHS_PER_YEAR[metonic_year as usize];
        molad_day += molad_halakim / HALAKIM_PER_DAY;
        molad_halakim %= HALAKIM_PER_DAY;
        metonic_year += 1;
    }

    TishriMolad {
        metonic_cycle,
        metonic_year,
        molad_day,
        molad_halakim,
    }
}

/// Result of [`find_start_of_year`].
struct StartOfYear {
    metonic_year: i32,
    molad_day: i64,
    molad_halakim: i64,
    /// Stored in an `int` by the C code.
    tishri1: i32,
}

/// `FindStartOfYear()`: the first day of `year` and its starting molad.
fn find_start_of_year(year: i32) -> StartOfYear {
    let metonic_cycle = (year - 1) / 19;
    let metonic_year = (year - 1) % 19;
    let (mut molad_day, mut molad_halakim) = molad_of_metonic_cycle(metonic_cycle);

    molad_halakim += HALAKIM_PER_LUNAR_CYCLE * YEAR_OFFSET[metonic_year as usize];
    molad_day += molad_halakim / HALAKIM_PER_DAY;
    molad_halakim %= HALAKIM_PER_DAY;

    StartOfYear {
        metonic_year,
        molad_day,
        molad_halakim,
        tishri1: tishri_1(metonic_year, molad_day, molad_halakim) as i32,
    }
}

/// `SdnToJewish()`: the all-zero date before year 1 and beyond
/// `JEWISH_SDN_MAX`.
pub fn sdn_to_jewish(sdn: i64) -> CalDate {
    if sdn <= JEWISH_SDN_OFFSET || sdn > JEWISH_SDN_MAX {
        return CalDate::ZERO;
    }
    let (year, month, day) = convert(sdn - JEWISH_SDN_OFFSET);
    CalDate {
        year: year as i32,
        month: month as i32,
        day: day as i32,
    }
}

/// The body of `SdnToJewish()` for `input_day = sdn - JEWISH_SDN_OFFSET`,
/// returning `(year, month, day)`.
fn convert(input_day: i64) -> (i64, i64, i64) {
    let found = find_tishri_molad(input_day);
    let (metonic_cycle, metonic_year) = (found.metonic_cycle, found.metonic_year);
    let (mut day, mut halakim) = (found.molad_day, found.molad_halakim);
    let mut tishri1 = i64::from(tishri_1(metonic_year, day, halakim) as i32);
    let year;
    let tishri1_after;

    if input_day >= tishri1 {
        // Found Tishri 1 at the start of the year.
        year = i64::from(metonic_cycle) * 19 + i64::from(metonic_year) + 1;
        if input_day < tishri1 + 59 {
            return if input_day < tishri1 + 30 {
                (year, 1, input_day - tishri1 + 1)
            } else {
                (year, 2, input_day - tishri1 - 29)
            };
        }
        // The length of the year is needed: find Tishri 1 of the next year.
        halakim += HALAKIM_PER_LUNAR_CYCLE * MONTHS_PER_YEAR[metonic_year as usize];
        day += halakim / HALAKIM_PER_DAY;
        halakim %= HALAKIM_PER_DAY;
        tishri1_after = i64::from(tishri_1((metonic_year + 1) % 19, day, halakim) as i32);
    } else {
        // Found Tishri 1 at the end of the year.
        year = i64::from(metonic_cycle) * 19 + i64::from(metonic_year);
        if input_day >= tishri1 - 177 {
            // One of the last 6 months of the year.
            return if input_day > tishri1 - 30 {
                (year, 13, input_day - tishri1 + 30)
            } else if input_day > tishri1 - 60 {
                (year, 12, input_day - tishri1 + 60)
            } else if input_day > tishri1 - 89 {
                (year, 11, input_day - tishri1 + 89)
            } else if input_day > tishri1 - 119 {
                (year, 10, input_day - tishri1 + 119)
            } else if input_day > tishri1 - 148 {
                (year, 9, input_day - tishri1 + 148)
            } else {
                (year, 8, input_day - tishri1 + 178)
            };
        }
        let mut month;
        let mut dd;
        if months_per_year(((year - 1) % 19) as i32) == 13 {
            month = 7;
            dd = input_day - tishri1 + 207;
            if dd > 0 {
                return (year, month, dd);
            }
            month -= 1;
            dd += 30;
            if dd > 0 {
                return (year, month, dd);
            }
            month -= 1;
            dd += 30;
        } else {
            month = 7;
            dd = input_day - tishri1 + 207;
            if dd > 0 {
                return (year, month, dd);
            }
            month -= 2;
            dd += 30;
        }
        if dd > 0 {
            return (year, month, dd);
        }
        month -= 1;
        dd += 29;
        if dd > 0 {
            return (year, month, dd);
        }

        // The length of the year is needed: find Tishri 1 of this year.
        tishri1_after = tishri1;
        let found = find_tishri_molad(day - 365);
        tishri1 = i64::from(
            tishri_1(found.metonic_year, found.molad_day, found.molad_halakim) as i32,
        );
    }

    let year_length = i64::from((tishri1_after - tishri1) as i32);
    let mut day = input_day - tishri1 - 29;
    if year_length == 355 || year_length == 385 {
        // Heshvan has 30 days.
        if day <= 30 {
            return (year, 2, day);
        }
        day -= 30;
    } else {
        // Heshvan has 29 days.
        if day <= 29 {
            return (year, 2, day);
        }
        day -= 29;
    }

    // It has to be Kislev.
    (year, 3, day)
}

/// `JewishToSdn()`: 0 for invalid dates.
pub fn jewish_to_sdn(year: i64, month: i64, day: i64) -> i64 {
    if year <= 0 || year >= i64::from(i32::MAX) - 1 || day <= 0 || day > 30 {
        return 0;
    }
    // The check above leaves both year and year + 1 within int range.
    let int_year = year as i32;

    let sdn = match month {
        1 | 2 => {
            // Tishri or Heshvan: the year length is not needed.
            let start = find_start_of_year(int_year);
            if month == 1 {
                i64::from(start.tishri1) + day - 1
            } else {
                i64::from(start.tishri1) + day + 29
            }
        }
        3 => {
            // Kislev: the year length is needed.
            let start = find_start_of_year(int_year);

            // Find the end of the year.
            let mut molad_halakim = start.molad_halakim
                + HALAKIM_PER_LUNAR_CYCLE * MONTHS_PER_YEAR[start.metonic_year as usize];
            let molad_day = start.molad_day + molad_halakim / HALAKIM_PER_DAY;
            molad_halakim %= HALAKIM_PER_DAY;
            let tishri1_after =
                tishri_1((start.metonic_year + 1) % 19, molad_day, molad_halakim) as i32;

            let year_length = tishri1_after.wrapping_sub(start.tishri1);

            if year_length == 355 || year_length == 385 {
                i64::from(start.tishri1) + day + 59
            } else {
                i64::from(start.tishri1) + day + 58
            }
        }
        4..=6 => {
            // Tevet, Shevat or Adar I: the year length is not needed.
            let next = find_start_of_year(int_year + 1);
            let length_of_adar_i_and_ii = if months_per_year((int_year - 1) % 19) == 12 {
                29
            } else {
                59
            };
            let after = i64::from(next.tishri1);
            match month {
                4 => after + day - length_of_adar_i_and_ii - 237,
                5 => after + day - length_of_adar_i_and_ii - 208,
                _ => after + day - length_of_adar_i_and_ii - 178,
            }
        }
        _ => {
            // Adar II or later: the year length is not needed.
            let next = find_start_of_year(int_year + 1);
            let after = i64::from(next.tishri1);
            match month {
                7 => after + day - 207,
                8 => after + day - 178,
                9 => after + day - 148,
                10 => after + day - 119,
                11 => after + day - 89,
                12 => after + day - 60,
                13 => after + day - 30,
                _ => return 0,
            }
        }
    };
    sdn + JEWISH_SDN_OFFSET
}
