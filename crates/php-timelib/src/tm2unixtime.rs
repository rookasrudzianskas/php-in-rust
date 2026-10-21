//! Broken-down time to Unix timestamp, relative adjustments and normalisation
//! (`tm2unixtime.c`).

use std::sync::Arc;

use crate::dow::{day_of_week, is_leap};
use crate::time::{
    DAYS_PER_ERA, HINNANT_EPOCH_SHIFT, RelTime, SECS_PER_DAY, SECS_PER_HOUR,
    SPECIAL_DAY_OF_WEEK_IN_MONTH, SPECIAL_FIRST_DAY_OF_MONTH, SPECIAL_LAST_DAY_OF_MONTH,
    SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH, SPECIAL_WEEKDAY, SpecialRelative, Time, UNSET,
    YEARS_PER_ERA, ZoneType, hms_to_seconds,
};
use crate::tz::{TzInfo, offset_info};
use crate::unixtime2tm::date_from_epoch_days;

//                                     dec  jan  feb  mar  apr  may  jun  jul  aug  sep  oct  nov  dec
const DAYS_IN_MONTH_LEAP: [i64; 13] = [31, 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DAYS_IN_MONTH: [i64; 13] = [31, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

fn month_days(leap: bool, m: i64) -> i64 {
    let idx = m.clamp(0, 12) as usize;
    if leap {
        DAYS_IN_MONTH_LEAP[idx]
    } else {
        DAYS_IN_MONTH[idx]
    }
}

/// Brings `*a` into `[start, end)` by carrying into `*b` (`do_range_limit`).
pub(crate) fn do_range_limit(start: i64, end: i64, adj: i64, a: &mut i64, b: &mut i64) {
    if *a < start {
        // 'a + 1' first, as 'start - a - 1' overflows when a is i64::MIN.
        let a_plus_1 = a.wrapping_add(1);
        *b = b.wrapping_sub(start.wrapping_sub(a_plus_1) / adj + 1);
        *a = a.wrapping_add(adj.wrapping_mul(start.wrapping_sub(a_plus_1) / adj));
        *a = a.wrapping_add(adj);
    }
    if *a >= end {
        *b = b.wrapping_add(*a / adj);
        *a = a.wrapping_sub(adj.wrapping_mul(*a / adj));
    }
}

fn inc_month(y: &mut i64, m: &mut i64) {
    *m += 1;
    if *m > 12 {
        *m -= 12;
        *y += 1;
    }
}

fn dec_month(y: &mut i64, m: &mut i64) {
    *m -= 1;
    if *m < 1 {
        *m += 12;
        *y -= 1;
    }
}

fn do_range_limit_days_relative(
    base_y: &mut i64,
    base_m: &mut i64,
    m: &mut i64,
    d: &mut i64,
    invert: bool,
) {
    do_range_limit(1, 13, 12, base_m, base_y);
    let mut year = *base_y;
    let mut month = *base_m;
    if !invert {
        while *d < 0 {
            dec_month(&mut year, &mut month);
            *d += month_days(is_leap(year), month);
            *m -= 1;
        }
    } else {
        while *d < 0 {
            *d += month_days(is_leap(year), month);
            *m -= 1;
            inc_month(&mut year, &mut month);
        }
    }
}

fn do_range_limit_days(y: &mut i64, m: &mut i64, d: &mut i64) -> bool {
    // can jump an entire leap year period quickly
    if *d >= DAYS_PER_ERA || *d <= -DAYS_PER_ERA {
        *y = y.wrapping_add(YEARS_PER_ERA.wrapping_mul(*d / DAYS_PER_ERA));
        *d -= DAYS_PER_ERA * (*d / DAYS_PER_ERA);
    }
    do_range_limit(1, 13, 12, m, y);

    let leap_current = is_leap(*y);
    let mut retval = false;
    while *d <= 0 && *m > 0 {
        let (previous_month, previous_year) = if *m - 1 < 1 {
            (*m - 1 + 12, y.wrapping_sub(1))
        } else {
            (*m - 1, *y)
        };
        *d += month_days(is_leap(previous_year), previous_month);
        *m -= 1;
        retval = true;
    }
    while *d > 0 && *m <= 12 && *d > month_days(leap_current, *m) {
        *d -= month_days(leap_current, *m);
        *m += 1;
        retval = true;
    }
    retval
}

impl Time {
    fn do_adjust_for_weekday(&mut self) {
        let current_dow = day_of_week(self.y, self.m, self.d);
        let rel = &mut self.relative;
        if rel.weekday_behavior == 2 {
            // To make "this week" work, where the current DOW is a "sunday"
            if current_dow == 0 && rel.weekday != 0 {
                rel.weekday -= 7;
            }
            // To make "sunday this week" work, where the current DOW is not a "sunday"
            if rel.weekday == 0 && current_dow != 0 {
                rel.weekday = 7;
            }
            self.d -= current_dow;
            self.d += i64::from(rel.weekday);
            return;
        }
        let mut difference = i64::from(rel.weekday) - current_dow;
        if (rel.d < 0 && difference < 0)
            || (rel.d >= 0 && difference <= -i64::from(rel.weekday_behavior))
        {
            difference += 7;
        }
        if rel.weekday >= 0 {
            self.d += difference;
        } else {
            self.d -= 7 - (i64::from(rel.weekday.unsigned_abs()) - current_dow);
        }
        rel.have_weekday_relative = false;
    }

    /// Normalises out-of-range fields (`timelib_do_normalize`).
    pub fn do_normalize(&mut self) {
        if self.us != UNSET {
            do_range_limit(0, 1_000_000, 1_000_000, &mut self.us, &mut self.s);
        }
        if self.s != UNSET {
            do_range_limit(0, 60, 60, &mut self.s, &mut self.i);
        }
        if self.s != UNSET {
            do_range_limit(0, 60, 60, &mut self.i, &mut self.h);
        }
        if self.s != UNSET {
            do_range_limit(0, 24, 24, &mut self.h, &mut self.d);
        }
        do_range_limit(1, 13, 12, &mut self.m, &mut self.y);

        // Short cut if we're doing things against the Epoch
        if self.y == 1970 && self.m == 1 {
            let (y, m, d) = date_from_epoch_days(self.d.wrapping_sub(1));
            self.y = y;
            self.m = m;
            self.d = d;
            return;
        }

        while do_range_limit_days(&mut self.y, &mut self.m, &mut self.d) {}
        do_range_limit(1, 13, 12, &mut self.m, &mut self.y);
    }

    fn do_adjust_relative(&mut self) {
        if self.relative.have_weekday_relative {
            self.do_adjust_for_weekday();
        }
        self.do_normalize();

        if self.have_relative {
            let r = self.relative;
            self.us = self.us.wrapping_add(r.us);
            self.s = self.s.wrapping_add(r.s);
            self.i = self.i.wrapping_add(r.i);
            self.h = self.h.wrapping_add(r.h);
            self.d = self.d.wrapping_add(r.d);
            self.m = self.m.wrapping_add(r.m);
            self.y = self.y.wrapping_add(r.y);
        }

        match self.relative.first_last_day_of {
            SPECIAL_FIRST_DAY_OF_MONTH => self.d = 1,
            SPECIAL_LAST_DAY_OF_MONTH => {
                self.d = 0;
                self.m = self.m.wrapping_add(1);
            }
            _ => {}
        }

        self.do_normalize();
    }

    fn do_adjust_special_weekday(&mut self) {
        let count = self.relative.special.amount;
        let dow = day_of_week(self.y, self.m, self.d);

        // Add increments of 5 weekdays as a week, leaving the DOW unchanged.
        self.d = self.d.wrapping_add((count / 5).wrapping_mul(7));

        // Deal with the remainder.
        let rem = count % 5;
        if count > 0 {
            if rem == 0 {
                // Head back to Friday if we stop on the weekend.
                if dow == 0 {
                    self.d -= 2;
                } else if dow == 6 {
                    self.d -= 1;
                }
            } else if dow == 6 {
                // We ended up on Saturday, but there's still work to do, so move to
                // Sunday and continue from there.
                self.d += 1;
            } else if dow + rem > 5 {
                // We're on a weekday, but we're going past Friday, so skip right over
                // the weekend.
                self.d += 2;
            }
        } else {
            // Completely mirror the forward direction. This also covers the 0 case.
            if rem == 0 {
                if dow == 6 {
                    self.d += 2;
                } else if dow == 0 {
                    self.d += 1;
                }
            } else if dow == 0 {
                self.d -= 1;
            } else if dow + rem < 1 {
                self.d -= 2;
            }
        }
        self.d += rem;
    }

    fn do_adjust_special(&mut self) {
        if self.relative.have_special_relative && self.relative.special.kind == SPECIAL_WEEKDAY {
            self.do_adjust_special_weekday();
        }
        self.do_normalize();
        self.relative.special = SpecialRelative::default();
    }

    fn do_adjust_special_early(&mut self) {
        if self.relative.have_special_relative {
            match self.relative.special.kind {
                SPECIAL_DAY_OF_WEEK_IN_MONTH => {
                    self.d = 1;
                    self.m = self.m.wrapping_add(self.relative.m);
                    self.relative.m = 0;
                }
                SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH => {
                    self.d = 1;
                    self.m = self.m.wrapping_add(self.relative.m).wrapping_add(1);
                    self.relative.m = 0;
                }
                _ => {}
            }
        }
        match self.relative.first_last_day_of {
            SPECIAL_FIRST_DAY_OF_MONTH => self.d = 1,
            SPECIAL_LAST_DAY_OF_MONTH => {
                self.d = 0;
                self.m = self.m.wrapping_add(1);
            }
            _ => {}
        }
        self.do_normalize();
    }

    fn do_adjust_timezone(&mut self, tzi: Option<&Arc<TzInfo>>) {
        match self.zone_type {
            ZoneType::Offset => {
                self.is_localtime = true;
                self.sse = self.sse.wrapping_sub(i64::from(self.z));
            }
            ZoneType::Abbr => {
                self.is_localtime = true;
                self.sse = self
                    .sse
                    .wrapping_sub(i64::from(self.z))
                    .wrapping_sub(i64::from(self.dst) * SECS_PER_HOUR);
            }
            ZoneType::Id | ZoneType::None => {
                // No timezone in struct, fallback to reference if possible
                let tzi: Option<Arc<TzInfo>> = if self.zone_type == ZoneType::Id {
                    self.tz_info.clone()
                } else {
                    tzi.cloned()
                };
                let Some(tzi) = tzi else {
                    return;
                };
                let (current_offset, current_transition_time, current_is_dst) =
                    match offset_info(Some(&tzi), self.sse) {
                        Some(o) => (i64::from(o.offset), o.transition_time, o.is_dst),
                        None => (0, 0, false),
                    };
                let _ = current_transition_time;
                let (after_offset, after_transition_time) =
                    match offset_info(Some(&tzi), self.sse.wrapping_sub(current_offset)) {
                        Some(o) => (i64::from(o.offset), o.transition_time),
                        None => (0, 0),
                    };
                let mut actual_offset = after_offset;
                let mut actual_transition_time = after_transition_time;
                if current_offset == after_offset && self.have_zone != 0 {
                    // Make sure we're not missing a DST change because we don't know the
                    // actual offset yet
                    if current_offset >= 0 && self.dst != 0 && !current_is_dst {
                        // Timezone or its DST at or east of UTC, so the local time,
                        // interpreted as UTC, leaves DST (as defined in the actual
                        // timezone) before the actual local time
                        let (earlier_offset, earlier_transition_time) = match offset_info(
                            Some(&tzi),
                            self.sse.wrapping_sub(current_offset).wrapping_sub(7200),
                        ) {
                            Some(o) => (i64::from(o.offset), o.transition_time),
                            None => (0, 0),
                        };
                        if earlier_offset != after_offset
                            && self.sse.wrapping_sub(earlier_offset) < after_transition_time
                        {
                            // Looking behind a bit clarified the actual offset to use
                            actual_offset = earlier_offset;
                            actual_transition_time = earlier_transition_time;
                        }
                    } else if current_offset <= 0 && current_is_dst && self.dst == 0 {
                        // Timezone west of UTC, so the local time, interpreted as UTC,
                        // leaves DST (as defined in the actual timezone) after the actual
                        // local time
                        let (later_offset, later_transition_time) = match offset_info(
                            Some(&tzi),
                            self.sse.wrapping_sub(current_offset).wrapping_add(7200),
                        ) {
                            Some(o) => (i64::from(o.offset), o.transition_time),
                            None => (0, 0),
                        };
                        if later_offset != after_offset
                            && self.sse.wrapping_sub(later_offset) >= later_transition_time
                        {
                            // Looking ahead a bit clarified the actual offset to use
                            actual_offset = later_offset;
                            actual_transition_time = later_transition_time;
                        }
                    }
                }

                self.is_localtime = true;

                let local = self.sse.wrapping_sub(actual_offset);
                let in_transition = actual_transition_time != i64::MIN
                    && local >= actual_transition_time.wrapping_add(current_offset - actual_offset)
                    && local < actual_transition_time;

                let adjustment = if current_offset != actual_offset && !in_transition {
                    -actual_offset
                } else {
                    -current_offset
                };
                self.sse = self.sse.wrapping_add(adjustment);
                self.set_timezone(&tzi);
            }
        }
    }

    /// Days since 1970-01-01 of the date part (`timelib_epoch_days_from_time`; Howard
    /// Hinnant's `days_from_civil`).
    pub fn epoch_days(&self) -> i64 {
        let y = self.y.wrapping_sub(i64::from(self.m <= 2));
        let era = if y >= 0 { y } else { y.wrapping_sub(399) } / YEARS_PER_ERA;
        let year_of_era = y.wrapping_sub(era.wrapping_mul(YEARS_PER_ERA));
        let day_of_year = (153 * (self.m + if self.m > 2 { -3 } else { 9 }) + 2) / 5 + self.d - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era.wrapping_mul(DAYS_PER_ERA)
            .wrapping_add(day_of_era)
            .wrapping_sub(HINNANT_EPOCH_SHIFT)
    }

    /// Applies the relative part and the zone, and computes `sse` (`timelib_update_ts`).
    /// `tzi` is the fallback zone used when the time itself has no zone information.
    pub fn update_ts(&mut self, tzi: Option<&Arc<TzInfo>>) {
        self.do_adjust_special_early();
        self.do_adjust_relative();
        self.do_adjust_special();

        // Computed in two halves so that the earliest representable day does not
        // overflow.
        let days = self.epoch_days();
        self.sse = hms_to_seconds(self.h, self.i, self.s);
        self.sse = self.sse.wrapping_add(days.wrapping_mul(SECS_PER_DAY / 2));
        self.sse = self.sse.wrapping_add(days.wrapping_mul(SECS_PER_DAY / 2));

        self.do_adjust_timezone(tzi);

        self.sse_uptodate = true;
        self.have_relative = false;
        self.relative.have_weekday_relative = false;
        self.relative.have_special_relative = false;
        self.relative.first_last_day_of = 0;
    }
}

/// Normalises a relative time against a base time (`timelib_do_rel_normalize`).
pub fn do_rel_normalize(base: &mut Time, rt: &mut RelTime) {
    do_range_limit(0, 1_000_000, 1_000_000, &mut rt.us, &mut rt.s);
    do_range_limit(0, 60, 60, &mut rt.s, &mut rt.i);
    do_range_limit(0, 60, 60, &mut rt.i, &mut rt.h);
    do_range_limit(0, 24, 24, &mut rt.h, &mut rt.d);
    do_range_limit(0, 12, 12, &mut rt.m, &mut rt.y);

    do_range_limit_days_relative(&mut base.y, &mut base.m, &mut rt.m, &mut rt.d, rt.invert);
    do_range_limit(0, 12, 12, &mut rt.m, &mut rt.y);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(y: i64, m: i64, d: i64, h: i64, i: i64, s: i64) -> Time {
        Time {
            y,
            m,
            d,
            h,
            i,
            s,
            ..Time::new()
        }
    }

    #[test]
    fn utc_update_ts() {
        let mut t = mk(2009, 2, 13, 23, 31, 30);
        t.update_ts(None);
        assert_eq!(t.sse, 1_234_567_890);
        let mut t = mk(1969, 12, 31, 23, 59, 59);
        t.update_ts(None);
        assert_eq!(t.sse, -1);
    }

    #[test]
    fn normalize_overflow() {
        let mut t = mk(2005, 14, 35, 25, 61, 61);
        t.update_ts(None);
        let mut u = Time::new();
        u.unixtime2gmt(t.sse);
        assert_eq!((u.y, u.m, u.d, u.h, u.i, u.s), (2006, 3, 8, 2, 2, 1));
    }

    #[test]
    fn with_zone() {
        let tz = crate::tz::builtin_tzinfo("Europe/Amsterdam").unwrap();
        let mut t = mk(2005, 7, 1, 14, 0, 0);
        t.update_ts(Some(&tz));
        assert_eq!(t.sse, 1_120_219_200);
        assert_eq!(t.tz_abbr.as_deref(), Some("CEST"));
        assert_eq!(t.z, 7200);
    }
}
