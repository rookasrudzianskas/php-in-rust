//! `date()` / `DateTime::format()` / `idate()` / `DateInterval::format()` formatting
//! (ported from `ext/date/php_date.c`).

use std::fmt::Write as _;

use crate::dow::{
    day_of_week, day_of_year, days_in_month, is_leap, iso_day_of_week, isoweek_from_date,
};
use crate::time::{RelTime, Time, TimeOffset, UNSET, ZoneType};

/// Full month names.
pub const MON_FULL_NAMES: [&str; 12] = [
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

/// Abbreviated month names.
pub const MON_SHORT_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Full day names, Sunday first.
pub const DAY_FULL_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Abbreviated day names, Sunday first.
pub const DAY_SHORT_NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// English ordinal suffix for a day number (`english_suffix`): "st", "nd", "rd", "th".
pub fn english_suffix(number: i64) -> &'static str {
    if (10..=19).contains(&number) {
        return "th";
    }
    match number % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

/// Full day name for a date (`php_date_full_day_name`).
pub fn full_day_name(y: i64, m: i64, d: i64) -> &'static str {
    usize::try_from(day_of_week(y, m, d))
        .ok()
        .and_then(|i| DAY_FULL_NAMES.get(i))
        .copied()
        .unwrap_or("Unknown")
}

/// Short day name for a date (`php_date_short_day_name`).
pub fn short_day_name(y: i64, m: i64, d: i64) -> &'static str {
    usize::try_from(day_of_week(y, m, d))
        .ok()
        .and_then(|i| DAY_SHORT_NAMES.get(i))
        .copied()
        .unwrap_or("Unknown")
}

fn month_name(names: &[&'static str; 12], m: i64) -> &'static str {
    usize::try_from(m - 1)
        .ok()
        .and_then(|i| names.get(i))
        .copied()
        .unwrap_or("")
}

/// Swatch Internet Time (`B`) for a timestamp.
pub fn swatch_beat(sse: i64) -> i32 {
    let mut retval = (((sse % 86400) + 3600) * 10) as i32;
    if retval < 0 {
        retval += 864_000;
    }
    (retval / 864) % 1000
}

/// `+HH:MM` / `+HH:MM:SS` offset string (`date_create_tz_offset_str`).
pub fn tz_offset_str(offset: i64) -> String {
    let seconds = (offset % 60) as i32;
    let sign = if offset < 0 { '-' } else { '+' };
    let h = ((offset / 3600) as i32).abs();
    let m = (((offset % 3600) as i32) / 60).abs();
    if seconds == 0 {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!("{sign}{h:02}:{m:02}:{:02}", seconds.abs())
    }
}

/// The offset information `date()` uses for a local time.
pub fn local_offset(t: &Time) -> TimeOffset {
    match t.zone_type {
        ZoneType::Abbr => TimeOffset {
            offset: t.z + t.dst * 3600,
            leap_secs: 0,
            is_dst: t.dst != 0,
            abbr: t.tz_abbr.clone().unwrap_or_default(),
            transition_time: 0,
        },
        ZoneType::Offset => {
            let offset = t.z;
            TimeOffset {
                offset,
                leap_secs: 0,
                is_dst: false,
                abbr: format!(
                    "GMT{}{:02}{:02}",
                    if offset < 0 { '-' } else { '+' },
                    (offset / 3600).abs(),
                    ((offset % 3600) / 60).abs()
                ),
                transition_time: 0,
            }
        }
        ZoneType::Id => t
            .tz_info
            .as_deref()
            .map(|tz| tz.get_time_zone_info(t.sse))
            .unwrap_or_default(),
        ZoneType::None => TimeOffset::default(),
    }
}

/// Formats `t` with PHP `date()` format characters (`date_format`). With
/// `localtime == false` the time is treated as UTC (as `gmdate()` does).
#[allow(clippy::too_many_lines)]
pub fn date_format(format: &[u8], t: &Time, localtime: bool) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(format.len() * 2);
    if format.is_empty() {
        return out;
    }
    let offset = if localtime {
        local_offset(t)
    } else {
        TimeOffset::default()
    };
    let mut iso: Option<(i64, i64)> = None;
    let mut buf = String::new();

    let mut i = 0;
    while i < format.len() {
        buf.clear();
        let mut rfc_colon = false;
        let ch = format[i];
        match ch {
            // day
            b'd' => {
                let _ = write!(buf, "{:02}", t.d as i32);
            }
            b'D' => buf.push_str(short_day_name(t.y, t.m, t.d)),
            b'j' => {
                let _ = write!(buf, "{}", t.d as i32);
            }
            b'l' => buf.push_str(full_day_name(t.y, t.m, t.d)),
            b'S' => buf.push_str(english_suffix(t.d)),
            b'w' => {
                let _ = write!(buf, "{}", day_of_week(t.y, t.m, t.d) as i32);
            }
            b'N' => {
                let _ = write!(buf, "{}", iso_day_of_week(t.y, t.m, t.d) as i32);
            }
            b'z' => {
                let _ = write!(buf, "{}", day_of_year(t.y, t.m, t.d) as i32);
            }
            // week
            b'W' => {
                let (w, _) = *iso.get_or_insert_with(|| isoweek_from_date(t.y, t.m, t.d));
                let _ = write!(buf, "{:02}", w as i32);
            }
            b'o' => {
                let (_, y) = *iso.get_or_insert_with(|| isoweek_from_date(t.y, t.m, t.d));
                let _ = write!(buf, "{y}");
            }
            // month
            b'F' => buf.push_str(month_name(&MON_FULL_NAMES, t.m)),
            b'm' => {
                let _ = write!(buf, "{:02}", t.m as i32);
            }
            b'M' => buf.push_str(month_name(&MON_SHORT_NAMES, t.m)),
            b'n' => {
                let _ = write!(buf, "{}", t.m as i32);
            }
            b't' => {
                let _ = write!(buf, "{}", days_in_month(t.y, t.m) as i32);
            }
            // year
            b'L' => {
                let _ = write!(buf, "{}", i32::from(is_leap(i64::from(t.y as i32))));
            }
            b'y' => {
                let _ = write!(buf, "{:02}", (t.y % 100) as i32);
            }
            b'Y' => {
                let _ = write!(
                    buf,
                    "{}{:04}",
                    if t.y < 0 { "-" } else { "" },
                    t.y.unsigned_abs()
                );
            }
            b'x' => {
                let sign = if t.y < 0 {
                    "-"
                } else if t.y >= 10000 {
                    "+"
                } else {
                    ""
                };
                let _ = write!(buf, "{sign}{:04}", t.y.unsigned_abs());
            }
            b'X' => {
                let _ = write!(
                    buf,
                    "{}{:04}",
                    if t.y < 0 { "-" } else { "+" },
                    t.y.unsigned_abs()
                );
            }
            // time
            b'a' => buf.push_str(if t.h >= 12 { "pm" } else { "am" }),
            b'A' => buf.push_str(if t.h >= 12 { "PM" } else { "AM" }),
            b'B' => {
                let _ = write!(buf, "{:03}", swatch_beat(t.sse));
            }
            b'g' => {
                let h = if t.h % 12 != 0 { (t.h % 12) as i32 } else { 12 };
                let _ = write!(buf, "{h}");
            }
            b'G' => {
                let _ = write!(buf, "{}", t.h as i32);
            }
            b'h' => {
                let h = if t.h % 12 != 0 { (t.h % 12) as i32 } else { 12 };
                let _ = write!(buf, "{h:02}");
            }
            b'H' => {
                let _ = write!(buf, "{:02}", t.h as i32);
            }
            b'i' => {
                let _ = write!(buf, "{:02}", t.i as i32);
            }
            b's' => {
                let _ = write!(buf, "{:02}", t.s as i32);
            }
            b'u' => {
                let _ = write!(buf, "{:06}", t.us as i32);
            }
            b'v' => {
                let _ = write!(buf, "{:03}", (t.us / 1000) as i32);
            }
            // timezone
            b'I' => {
                let _ = write!(buf, "{}", i32::from(localtime && offset.is_dst));
            }
            b'p' | b'P' | b'O' => {
                if ch == b'p'
                    && (!localtime
                        || offset.abbr == "UTC"
                        || offset.abbr == "Z"
                        || offset.abbr == "GMT+0000")
                {
                    buf.push('Z');
                } else {
                    if ch != b'O' {
                        rfc_colon = true;
                    }
                    let (sign, h, m) = if localtime {
                        (
                            if offset.offset < 0 { '-' } else { '+' },
                            (offset.offset / 3600).abs(),
                            ((offset.offset % 3600) / 60).abs(),
                        )
                    } else {
                        ('+', 0, 0)
                    };
                    let _ = write!(
                        buf,
                        "{sign}{h:02}{}{m:02}",
                        if rfc_colon { ":" } else { "" }
                    );
                }
            }
            b'T' => buf.push_str(if localtime { &offset.abbr } else { "GMT" }),
            b'e' => {
                if !localtime {
                    buf.push_str("UTC");
                } else {
                    match t.zone_type {
                        ZoneType::Id => buf.push_str(t.tz_name().unwrap_or("")),
                        ZoneType::Abbr => buf.push_str(&offset.abbr),
                        ZoneType::Offset => buf.push_str(&tz_offset_str(i64::from(offset.offset))),
                        ZoneType::None => {}
                    }
                }
            }
            b'Z' => {
                let _ = write!(buf, "{}", if localtime { offset.offset } else { 0 });
            }
            // full date/time
            b'c' => {
                let (sign, h, m) = if localtime {
                    (
                        if offset.offset < 0 { '-' } else { '+' },
                        (offset.offset / 3600).abs(),
                        ((offset.offset % 3600) / 60).abs(),
                    )
                } else {
                    ('+', 0, 0)
                };
                let _ = write!(
                    buf,
                    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{h:02}:{m:02}",
                    t.y, t.m as i32, t.d as i32, t.h as i32, t.i as i32, t.s as i32
                );
            }
            b'r' => {
                let (sign, h, m) = if localtime {
                    (
                        if offset.offset < 0 { '-' } else { '+' },
                        (offset.offset / 3600).abs(),
                        ((offset.offset % 3600) / 60).abs(),
                    )
                } else {
                    ('+', 0, 0)
                };
                let _ = write!(
                    buf,
                    "{:>3}, {:02} {:>3} {:04} {:02}:{:02}:{:02} {sign}{h:02}{m:02}",
                    short_day_name(t.y, t.m, t.d),
                    t.d as i32,
                    month_name(&MON_SHORT_NAMES, t.m),
                    t.y,
                    t.h as i32,
                    t.i as i32,
                    t.s as i32
                );
            }
            b'U' => {
                let _ = write!(buf, "{}", t.sse);
            }
            b'\\' => {
                i += 1;
                out.push(format.get(i).copied().unwrap_or(0));
                i += 1;
                continue;
            }
            other => {
                out.push(other);
                i += 1;
                continue;
            }
        }
        out.extend_from_slice(buf.as_bytes());
        i += 1;
    }
    out
}

/// Formats a timestamp like PHP's `date()` (`localtime = true`, in zone `tz`) or
/// `gmdate()` (`tz = None`), see `php_format_date`.
pub fn format_timestamp(
    format: &[u8],
    ts: i64,
    tz: Option<&std::sync::Arc<crate::tz::TzInfo>>,
) -> Vec<u8> {
    let mut t = Time::new();
    match tz {
        Some(tz) => {
            t.tz_info = Some(tz.clone());
            t.zone_type = ZoneType::Id;
            t.unixtime2local(ts);
            date_format(format, &t, true)
        }
        None => {
            t.unixtime2gmt(ts);
            date_format(format, &t, false)
        }
    }
}

/// `idate()` for a local time `t` (as built by `php_idate`). Returns `None` for an
/// unrecognised format character. Values are truncated to C `int`, like PHP.
pub fn idate(format: u8, t: &Time) -> Option<i32> {
    let offset = local_offset(t);
    let (iw, iy) = isoweek_from_date(t.y, t.m, t.d);
    Some(match format {
        b'd' | b'j' => t.d as i32,
        b'N' => iso_day_of_week(t.y, t.m, t.d) as i32,
        b'w' => day_of_week(t.y, t.m, t.d) as i32,
        b'z' => day_of_year(t.y, t.m, t.d) as i32,
        b'W' => iw as i32,
        b'm' | b'n' => t.m as i32,
        b't' => days_in_month(t.y, t.m) as i32,
        b'L' => i32::from(is_leap(i64::from(t.y as i32))),
        b'y' => (t.y % 100) as i32,
        b'Y' => t.y as i32,
        b'o' => iy as i32,
        b'B' => swatch_beat(t.sse),
        b'g' | b'h' => {
            if t.h % 12 != 0 {
                (t.h % 12) as i32
            } else {
                12
            }
        }
        b'H' | b'G' => t.h as i32,
        b'i' => t.i as i32,
        b's' => t.s as i32,
        b'I' => i32::from(offset.is_dst),
        b'Z' => offset.offset,
        b'U' => t.sse as i32,
        _ => return None,
    })
}

/// `DateInterval::format()` (`date_interval_format`).
pub fn interval_format(format: &[u8], t: &RelTime) -> Vec<u8> {
    let mut out = Vec::with_capacity(format.len() * 2);
    let mut have_spec = false;
    for &c in format {
        if have_spec {
            let s = match c {
                b'Y' => format!("{:02}", t.y as i32),
                b'y' => format!("{}", t.y as i32),
                b'M' => format!("{:02}", t.m as i32),
                b'm' => format!("{}", t.m as i32),
                b'D' => format!("{:02}", t.d as i32),
                b'd' => format!("{}", t.d as i32),
                b'H' => format!("{:02}", t.h as i32),
                b'h' => format!("{}", t.h as i32),
                b'I' => format!("{:02}", t.i as i32),
                b'i' => format!("{}", t.i as i32),
                b'S' => format!("{:02}", t.s),
                b's' => format!("{}", t.s),
                b'F' => format!("{:06}", t.us),
                b'f' => format!("{}", t.us),
                b'a' => {
                    if t.days as i32 != UNSET as i32 {
                        format!("{}", t.days as i32)
                    } else {
                        "(unknown)".to_string()
                    }
                }
                b'r' => (if t.invert { "-" } else { "" }).to_string(),
                b'R' => (if t.invert { "-" } else { "+" }).to_string(),
                b'%' => "%".to_string(),
                other => {
                    out.push(b'%');
                    out.push(other);
                    have_spec = false;
                    continue;
                }
            };
            out.extend_from_slice(s.as_bytes());
            have_spec = false;
        } else if c == b'%' {
            have_spec = true;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tz::builtin_tzinfo;

    #[test]
    fn gmdate_formats() {
        let s = format_timestamp(b"D, d M Y H:i:s", 1_234_567_890, None);
        assert_eq!(s, b"Fri, 13 Feb 2009 23:31:30");
        let s = format_timestamp(b"c|r|U|e|T|P|p|O|Z|I|B", 0, None);
        assert_eq!(
            String::from_utf8(s).unwrap(),
            "1970-01-01T00:00:00+00:00|Thu, 01 Jan 1970 00:00:00 +0000|0|UTC|GMT|+00:00|Z|+0000|0|0|041"
        );
        assert_eq!(format_timestamp(b"\\Y\\", 0, None), b"Y\0");
    }

    #[test]
    fn local_formats() {
        let tz = builtin_tzinfo("Europe/Amsterdam").unwrap();
        let s = format_timestamp(b"Y-m-d H:i:s T e P I", 1_120_219_200, Some(&tz));
        assert_eq!(
            String::from_utf8(s).unwrap(),
            "2005-07-01 14:00:00 CEST Europe/Amsterdam +02:00 1"
        );
        let s = format_timestamp(b"jS F, W o N z t L", 1_104_537_600, Some(&tz));
        assert_eq!(
            String::from_utf8(s).unwrap(),
            "1st January, 53 2004 6 0 31 0"
        );
    }

    #[test]
    fn interval() {
        let r = RelTime {
            y: 1,
            m: 2,
            d: 3,
            h: 4,
            i: 5,
            s: 6,
            us: 7,
            days: UNSET,
            invert: true,
            ..RelTime::default()
        };
        assert_eq!(
            interval_format(b"%Y-%M-%D %H:%I:%S.%F %a %R%r %% %x", &r),
            b"01-02-03 04:05:06.000007 (unknown) -- % %x"
        );
    }
}
