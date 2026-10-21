//! Differences between times and adding/subtracting intervals (`interval.c`).

use std::cmp::Ordering;

use crate::time::{
    RelTime, SECS_PER_DAY, SECS_PER_HOUR, Time, ZoneType, hms_to_seconds, hmsf_to_decimal_hour,
    tz_name,
};
use crate::tm2unixtime::do_rel_normalize;
use crate::tz::offset_info;

fn both_same_tzid(one: &Time, two: &Time) -> bool {
    one.zone_type == ZoneType::Id && two.zone_type == ZoneType::Id && tz_name(one) == tz_name(two)
}

/// Returns `(earlier, later, inverted)` (`sort_old_to_new`).
fn sort_old_to_new<'a>(one: &'a Time, two: &'a Time) -> (&'a Time, &'a Time, bool) {
    // If both are TIMELIB_ZONETYPE_ID times with the same TZID, compare the wall clock
    // fields; otherwise fall back to the SSE.
    let swap = if both_same_tzid(one, two) {
        (one.y, one.m, one.d, one.h, one.i, one.s, one.us)
            > (two.y, two.m, two.d, two.h, two.i, two.s, two.us)
    } else {
        one.sse > two.sse || (one.sse == two.sse && one.us > two.us)
    };
    if swap {
        (two, one, true)
    } else {
        (one, two, false)
    }
}

fn diff_with_tzid(one: &Time, two: &Time) -> RelTime {
    let mut rt = RelTime::new();
    let (one, two, inverted) = sort_old_to_new(one, two);
    rt.invert = inverted;

    // Calculate correction for UTC offset changes between first and second SSE
    let mut dst_corr = i64::from(two.z) - i64::from(one.z);
    let dst_h_corr = dst_corr / 3600;
    let dst_m_corr = (dst_corr % 3600) / 60;

    rt.y = two.y.wrapping_sub(one.y);
    rt.m = two.m.wrapping_sub(one.m);
    rt.d = two.d.wrapping_sub(one.d);
    rt.h = two.h.wrapping_sub(one.h);
    rt.i = two.i.wrapping_sub(one.i);
    rt.s = two.s.wrapping_sub(one.s);
    rt.us = two.us.wrapping_sub(one.us);

    rt.days = i64::from(diff_days(one, two));

    // Fall Back: Cater for transition period, where rt.invert is 0, but there are
    // negative numbers
    if two.sse < one.sse {
        let flipped =
            rt.i.wrapping_mul(60)
                .wrapping_add(rt.s)
                .wrapping_sub(dst_corr)
                .wrapping_abs();
        rt.h = flipped / SECS_PER_HOUR;
        rt.i = (flipped - rt.h * SECS_PER_HOUR) / 60;
        rt.s = flipped % 60;
        rt.invert = !rt.invert;
    }

    let mut base = if rt.invert { one.clone() } else { two.clone() };
    do_rel_normalize(&mut base, &mut rt);

    if one.dst == 1 && two.dst == 0 {
        // Fall Back
        if two.tz_info.is_some()
            && two.sse.wrapping_sub(one.sse).wrapping_add(dst_corr) < SECS_PER_DAY
        {
            rt.h -= dst_h_corr;
            rt.i -= dst_m_corr;
        }
    } else if one.dst == 0 && two.dst == 1 {
        // Spring Forward
        if let Some(info) = offset_info(two.tz_info.as_deref(), two.sse) {
            let trans = info.transition_time;
            let one_next = one.sse.wrapping_add(SECS_PER_DAY);
            if !((one_next > trans) && (one_next <= trans.wrapping_add(dst_corr)))
                && two.sse >= trans
                && (two.sse.wrapping_sub(one.sse).wrapping_add(dst_corr) % SECS_PER_DAY)
                    > two.sse.wrapping_sub(trans)
            {
                rt.h -= dst_h_corr;
                rt.i -= dst_m_corr;
            }
        }
    } else if two.sse.wrapping_sub(one.sse) >= SECS_PER_DAY {
        // Check whether we're in the period to the next transition time
        if let Some(info) = offset_info(
            two.tz_info.as_deref(),
            two.sse.wrapping_sub(i64::from(two.z)),
        ) {
            dst_corr = i64::from(one.z) - i64::from(info.offset);
            if two.sse >= info.transition_time.wrapping_sub(dst_corr)
                && two.sse < info.transition_time
            {
                rt.d -= 1;
                rt.h = 24;
            }
        }
    }
    rt
}

/// The difference between two times, as used by `DateTime::diff()` (`timelib_diff`).
pub fn diff(one: &Time, two: &Time) -> RelTime {
    if both_same_tzid(one, two) {
        return diff_with_tzid(one, two);
    }
    let mut rt = RelTime::new();
    let (one, two, inverted) = sort_old_to_new(one, two);
    rt.invert = inverted;

    rt.y = two.y.wrapping_sub(one.y);
    rt.m = two.m.wrapping_sub(one.m);
    rt.d = two.d.wrapping_sub(one.d);
    rt.h = two.h.wrapping_sub(one.h);
    if one.zone_type != ZoneType::Id {
        rt.h = rt.h.wrapping_add(i64::from(one.dst));
    }
    if two.zone_type != ZoneType::Id {
        rt.h = rt.h.wrapping_sub(i64::from(two.dst));
    }
    rt.i = two.i.wrapping_sub(one.i);
    rt.s = two
        .s
        .wrapping_sub(one.s)
        .wrapping_sub(i64::from(two.z))
        .wrapping_add(i64::from(one.z));
    rt.us = two.us.wrapping_sub(one.us);

    rt.days = i64::from(diff_days(one, two));

    let mut base = if rt.invert { one.clone() } else { two.clone() };
    do_rel_normalize(&mut base, &mut rt);
    rt
}

/// Number of full days between two times (`timelib_diff_days`).
pub fn diff_days(one: &Time, two: &Time) -> i32 {
    if one.same_timezone(two) {
        let (earliest, latest) = if one.compare(two) == Ordering::Less {
            (one, two)
        } else {
            (two, one)
        };
        let earliest_time = hmsf_to_decimal_hour(earliest.h, earliest.i, earliest.s, earliest.us);
        let latest_time = hmsf_to_decimal_hour(latest.h, latest.i, latest.s, latest.us);
        let mut days = one
            .epoch_days()
            .wrapping_sub(two.epoch_days())
            .wrapping_abs() as i32;
        if latest_time < earliest_time && days > 0 {
            days -= 1;
        }
        days
    } else {
        let ddays = (one.sse as f64 - two.sse as f64).floor().abs() / 86400.0;
        if ddays <= f64::from(i32::MAX) {
            ddays as i32
        } else {
            i32::MAX
        }
    }
}

fn scaled(interval: &RelTime, bias: i64) -> RelTime {
    RelTime {
        y: interval.y.wrapping_mul(bias),
        m: interval.m.wrapping_mul(bias),
        d: interval.d.wrapping_mul(bias),
        h: interval.h.wrapping_mul(bias),
        i: interval.i.wrapping_mul(bias),
        s: interval.s.wrapping_mul(bias),
        us: interval.us.wrapping_mul(bias),
        ..RelTime::default()
    }
}

/// Adds an interval using relative (wall clock field) arithmetic (`timelib_add`).
pub fn add(old_time: &Time, interval: &RelTime) -> Time {
    let mut t = old_time.clone();
    if interval.have_weekday_relative || interval.have_special_relative {
        t.relative = *interval;
    } else {
        let bias = if interval.invert { -1 } else { 1 };
        t.relative = scaled(interval, bias);
    }
    t.have_relative = true;
    t.sse_uptodate = false;
    t.update_ts(None);
    t.update_from_sse();
    t.have_relative = false;
    t
}

/// Subtracts an interval (`timelib_sub`).
pub fn sub(old_time: &Time, interval: &RelTime) -> Time {
    let mut t = old_time.clone();
    let bias: i64 = if interval.invert { -1 } else { 1 };
    t.relative = scaled(interval, -bias);
    t.have_relative = true;
    t.sse_uptodate = false;
    t.update_ts(None);
    t.update_from_sse();
    t.have_relative = false;
    t
}

/// `do_range_limit` variant from `interval.c`.
fn do_range_limit(start: i64, end: i64, adj: i64, a: &mut i64, b: &mut i64) {
    if *a < start {
        *b -= (start - *a - 1) / adj + 1;
        *a += adj * ((start - *a - 1) / adj + 1);
    }
    if *a >= end {
        *b += *a / adj;
        *a -= adj * (*a / adj);
    }
}

fn add_or_sub_wall(old_time: &Time, interval: &RelTime, sign: i64) -> Time {
    let mut t = old_time.clone();
    t.have_relative = true;
    t.sse_uptodate = false;

    if interval.have_weekday_relative || interval.have_special_relative {
        t.relative = *interval;
        t.update_ts(None);
        t.update_from_sse();
    } else {
        let bias: i64 = if interval.invert { -1 } else { 1 };
        t.relative = RelTime::default();
        t.relative.y = interval.y.wrapping_mul(bias).wrapping_mul(sign);
        t.relative.m = interval.m.wrapping_mul(bias).wrapping_mul(sign);
        t.relative.d = interval.d.wrapping_mul(bias).wrapping_mul(sign);

        if t.relative.y != 0 || t.relative.m != 0 || t.relative.d != 0 {
            t.update_ts(None);
        }

        if interval.us == 0 {
            let secs = hms_to_seconds(interval.h, interval.i, interval.s);
            t.sse = t
                .sse
                .wrapping_add(sign.wrapping_mul(bias).wrapping_mul(secs));
            t.update_from_sse();
        } else {
            let mut temp = *interval;
            do_range_limit(0, 1_000_000, 1_000_000, &mut temp.us, &mut temp.s);
            let secs = hms_to_seconds(temp.h, temp.i, temp.s);
            t.sse = t
                .sse
                .wrapping_add(sign.wrapping_mul(bias).wrapping_mul(secs));
            t.update_from_sse();
            t.us =
                t.us.wrapping_add(sign.wrapping_mul(temp.us).wrapping_mul(bias));
            t.do_normalize();
            t.update_ts(None);
        }
        t.do_normalize();
    }

    if t.zone_type == ZoneType::Id
        && let Some(tz) = t.tz_info.clone()
    {
        t.set_timezone(&tz);
    }
    t.have_relative = false;
    t
}

/// Adds an interval, applying the time portion in elapsed (wall) time
/// (`timelib_add_wall`; used by `DateTime::add()` in current PHP).
pub fn add_wall(old_time: &Time, interval: &RelTime) -> Time {
    add_or_sub_wall(old_time, interval, 1)
}

/// Subtracts an interval with wall-time semantics (`timelib_sub_wall`).
pub fn sub_wall(old_time: &Time, interval: &RelTime) -> Time {
    add_or_sub_wall(old_time, interval, -1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(y: i64, m: i64, d: i64, h: i64, i: i64, s: i64) -> Time {
        let mut t = Time {
            y,
            m,
            d,
            h,
            i,
            s,
            zone_type: ZoneType::Offset,
            is_localtime: true,
            ..Time::new()
        };
        t.update_ts(None);
        t
    }

    #[test]
    fn simple_diff() {
        let a = utc(2000, 1, 1, 0, 0, 0);
        let b = utc(2001, 3, 4, 5, 6, 7);
        let r = diff(&a, &b);
        assert_eq!((r.y, r.m, r.d, r.h, r.i, r.s), (1, 2, 3, 5, 6, 7));
        assert!(!r.invert);
        assert_eq!(r.days, 428);
        let r = diff(&b, &a);
        assert!(r.invert);
        assert_eq!((r.y, r.m, r.d), (1, 2, 3));
    }

    #[test]
    fn add_sub() {
        let a = utc(2000, 1, 31, 0, 0, 0);
        let iv = RelTime {
            m: 1,
            ..RelTime::new()
        };
        let b = add(&a, &iv);
        assert_eq!((b.y, b.m, b.d), (2000, 3, 2));
        let c = sub(&b, &iv);
        assert_eq!((c.y, c.m, c.d), (2000, 2, 2));
        let w = add_wall(
            &a,
            &RelTime {
                h: 25,
                ..RelTime::new()
            },
        );
        assert_eq!((w.y, w.m, w.d, w.h), (2000, 2, 1, 1));
    }
}
