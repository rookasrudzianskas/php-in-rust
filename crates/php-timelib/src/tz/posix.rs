//! POSIX TZ strings (`parse_posix.c`), used for timestamps after the last transition.

use crate::dow::is_leap;
use crate::time::{SECS_PER_DAY, UNSET};

/// Kind of a POSIX transition rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PosixTransKind {
    /// `Jn`: Julian day 1..365, February 29th is never counted.
    JulianNoFeb29,
    /// `n`: zero based Julian day 0..365, February 29th is counted.
    JulianFeb29,
    /// `Mm.w.d`: day `d` of week `w` of month `m`.
    Mwd {
        /// Month (1..12).
        month: i32,
        /// Week (1..5, 5 = last).
        week: i32,
        /// Day of week (0 = Sunday).
        dow: i32,
    },
}

/// One transition rule (`timelib_posix_trans_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PosixTransInfo {
    /// The rule kind.
    pub kind: PosixTransKind,
    /// Day value for the Julian kinds.
    pub days: i32,
    /// Seconds after local midnight at which the transition happens.
    pub hour: i64,
}

/// A parsed POSIX TZ string (`timelib_posix_str`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PosixStr {
    /// Standard time abbreviation.
    pub std: String,
    /// Standard UTC offset in seconds.
    pub std_offset: i64,
    /// DST abbreviation, if any.
    pub dst: Option<String>,
    /// DST UTC offset in seconds.
    pub dst_offset: i64,
    /// Start of DST.
    pub dst_begin: Option<PosixTransInfo>,
    /// End of DST.
    pub dst_end: Option<PosixTransInfo>,
    /// Index into the zone's type table for standard time.
    pub type_index_std_type: usize,
    /// Index into the zone's type table for DST.
    pub type_index_dst_type: usize,
}

/// Up to six transitions computed for years `y-1`, `y`, `y+1`
/// (`timelib_posix_transitions`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PosixTransitions {
    /// Number of used entries.
    pub count: usize,
    /// Transition times.
    pub times: [i64; 6],
    /// Type indices.
    pub types: [usize; 6],
}

struct Reader<'a> {
    s: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn cur(&self) -> u8 {
        self.s.get(self.p).copied().unwrap_or(0)
    }

    /// `"<" [+-]? .+? ">" | [A-Za-z]+`
    fn description(&mut self) -> Option<String> {
        if self.cur() == b'<' {
            self.p += 1;
            let begin = self.p;
            while self.cur() != 0 && self.cur() != b'>' {
                self.p += 1;
            }
            if self.cur() == 0 {
                return None;
            }
            let end = self.p;
            self.p += 1;
            if end - begin < 1 {
                return None;
            }
            Some(String::from_utf8_lossy(&self.s[begin..end]).into_owned())
        } else {
            let begin = self.p;
            while self.cur().is_ascii_alphabetic() {
                self.p += 1;
            }
            if self.p - begin < 1 {
                return None;
            }
            Some(String::from_utf8_lossy(&self.s[begin..self.p]).into_owned())
        }
    }

    fn sign(&mut self) -> i64 {
        match self.cur() {
            b'+' => {
                self.p += 1;
                1
            }
            b'-' => {
                self.p += 1;
                -1
            }
            _ => 1,
        }
    }

    /// `[0-9]+`, returning [`UNSET`] if no digits were found.
    fn number(&mut self) -> i64 {
        let begin = self.p;
        let mut acc: i32 = 0;
        while self.cur() == b'0' {
            self.p += 1;
        }
        while self.cur().is_ascii_digit() {
            acc = acc
                .wrapping_mul(10)
                .wrapping_add(i32::from(self.cur() - b'0'));
            self.p += 1;
        }
        if begin == self.p {
            return UNSET;
        }
        i64::from(acc)
    }

    /// `[+-]? [0-9]+ ( ":" [0-9]+ ( ":" [0-9]+ )? )?` — note the inverted sign.
    fn offset(&mut self) -> i64 {
        let bias = self.sign();
        let begin = self.p;
        let hours = self.number();
        if hours == UNSET {
            return hours;
        }
        let mut minutes = 0;
        let mut seconds = 0;
        if self.cur() == b':' {
            self.p += 1;
            minutes = self.number();
            if minutes == UNSET {
                return minutes;
            }
        }
        if self.cur() == b':' {
            self.p += 1;
            seconds = self.number();
            if seconds == UNSET {
                return seconds;
            }
        }
        if begin == self.p {
            return UNSET;
        }
        -bias * (hours * 3600 + minutes * 60 + seconds)
    }

    fn trans_spec(&mut self) -> Option<PosixTransInfo> {
        let mut info = PosixTransInfo {
            kind: PosixTransKind::JulianFeb29,
            days: 0,
            hour: 2 * 3600,
        };
        if self.cur() == b'M' {
            self.p += 1;
            let month = self.number();
            if month == UNSET || self.cur() != b'.' {
                return None;
            }
            self.p += 1;
            let week = self.number();
            if week == UNSET || self.cur() != b'.' {
                return None;
            }
            self.p += 1;
            let dow = self.number();
            if dow == UNSET {
                return None;
            }
            info.kind = PosixTransKind::Mwd {
                month: month as i32,
                week: week as i32,
                dow: dow as i32,
            };
        } else {
            if self.cur() == b'J' {
                info.kind = PosixTransKind::JulianNoFeb29;
                self.p += 1;
            }
            let days = self.number();
            if days == UNSET {
                return None;
            }
            info.days = days as i32;
        }
        if self.cur() == b'/' {
            self.p += 1;
            let hour = self.offset();
            if hour == UNSET {
                return None;
            }
            info.hour = -hour;
        }
        Some(info)
    }
}

/// Parses a POSIX TZ string (`timelib_parse_posix_str`). Type indices are left at 0.
pub fn parse_posix_str(posix: &str) -> Option<PosixStr> {
    let mut r = Reader {
        s: posix.as_bytes(),
        p: 0,
    };
    let std = r.description()?;
    let std_offset = r.offset();
    if std_offset == UNSET {
        return None;
    }
    let mut ps = PosixStr {
        std,
        std_offset,
        dst: None,
        dst_offset: 0,
        dst_begin: None,
        dst_end: None,
        type_index_std_type: 0,
        type_index_dst_type: 0,
    };
    if r.cur() == 0 {
        return Some(ps);
    }
    ps.dst_offset = ps.std_offset + 3600;
    ps.dst = Some(r.description()?);
    if r.cur() != b',' && r.cur() != 0 {
        ps.dst_offset = r.offset();
        if ps.dst_offset == UNSET {
            return None;
        }
    }
    if r.cur() != b',' {
        return None;
    }
    r.p += 1;
    ps.dst_begin = Some(r.trans_spec()?);
    if r.cur() != b',' {
        return None;
    }
    r.p += 1;
    ps.dst_end = Some(r.trans_spec()?);
    if r.cur() != 0 {
        return None;
    }
    Some(ps)
}

const MONTH_LENGTHS: [[i64; 12]; 2] = [
    [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
    [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
];

/// Seconds from the start of `year` to the transition day (adapted from tzcode's
/// `transtime`).
fn calc_transition(psi: &PosixTransInfo, year: i64) -> i64 {
    let leap_year = usize::from(is_leap(year));
    match psi.kind {
        PosixTransKind::JulianNoFeb29 => {
            let mut value = i64::from(psi.days) - 1;
            if leap_year == 1 && psi.days >= 60 {
                value += 1;
            }
            value * SECS_PER_DAY
        }
        PosixTransKind::JulianFeb29 => i64::from(psi.days) * SECS_PER_DAY,
        PosixTransKind::Mwd { month, week, dow } => {
            // Zeller's Congruence for the day-of-week of the first day of the month.
            let m1 = (month + 9) % 12 + 1;
            let yy0 = if month <= 2 { year - 1 } else { year } as i32;
            let yy1 = yy0 / 100;
            let yy2 = yy0 % 100;
            let mut fdow = ((26 * m1 - 2) / 10 + 1 + yy2 + yy2 / 4 + yy1 / 4 - 2 * yy1) % 7;
            if fdow < 0 {
                fdow += 7;
            }
            let mut d = dow - fdow;
            if d < 0 {
                d += 7;
            }
            let mlen = |m: i32| MONTH_LENGTHS[leap_year][(m - 1).clamp(0, 11) as usize];
            for _ in 1..week {
                if i64::from(d + 7) >= mlen(month) {
                    break;
                }
                d += 7;
            }
            let mut value = i64::from(d) * SECS_PER_DAY;
            for i in 0..(month - 1).clamp(0, 12) {
                value += MONTH_LENGTHS[leap_year][i as usize] * SECS_PER_DAY;
            }
            value
        }
    }
}

fn count_leap_years(y: i64) -> i64 {
    let y = y - 1;
    (y / 4) - (y / 100) + (y / 400)
}

/// Unix timestamp of January 1st, 00:00 UTC of `year` (`timelib_ts_at_start_of_year`).
pub fn ts_at_start_of_year(year: i64) -> i64 {
    let epoch_leap_years = count_leap_years(1970);
    let current_leap_years = count_leap_years(year);
    SECS_PER_DAY.wrapping_mul(
        (year - 1970)
            .wrapping_mul(365)
            .wrapping_add(current_leap_years)
            .wrapping_sub(epoch_leap_years),
    )
}

/// Appends the two DST transitions of `year` (`timelib_get_transitions_for_year`).
pub fn transitions_for_year(ps: &PosixStr, year: i64, transitions: &mut PosixTransitions) {
    let (Some(begin), Some(end)) = (ps.dst_begin.as_ref(), ps.dst_end.as_ref()) else {
        return;
    };
    if transitions.count + 2 > transitions.times.len() {
        return;
    }
    let year_begin_ts = ts_at_start_of_year(year);
    let trans_begin = year_begin_ts + calc_transition(begin, year) + begin.hour - ps.std_offset;
    let trans_end = year_begin_ts + calc_transition(end, year) + end.hour - ps.dst_offset;
    let c = transitions.count;
    if trans_begin < trans_end {
        transitions.times[c] = trans_begin;
        transitions.times[c + 1] = trans_end;
        transitions.types[c] = ps.type_index_dst_type;
        transitions.types[c + 1] = ps.type_index_std_type;
    } else {
        transitions.times[c + 1] = trans_begin;
        transitions.times[c] = trans_end;
        transitions.types[c + 1] = ps.type_index_dst_type;
        transitions.types[c] = ps.type_index_std_type;
    }
    transitions.count += 2;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_eu() {
        let p = parse_posix_str("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        assert_eq!(p.std, "CET");
        assert_eq!(p.std_offset, 3600);
        assert_eq!(p.dst.as_deref(), Some("CEST"));
        assert_eq!(p.dst_offset, 7200);
        assert_eq!(p.dst_end.unwrap().hour, 3 * 3600);
    }

    #[test]
    fn parse_numeric() {
        let p = parse_posix_str("<-03>3").unwrap();
        assert_eq!(p.std, "-03");
        assert_eq!(p.std_offset, -3 * 3600);
        assert!(p.dst.is_none());
        assert!(parse_posix_str("").is_none());
        assert!(parse_posix_str("EST5EDT").is_none());
    }

    #[test]
    fn start_of_year() {
        assert_eq!(ts_at_start_of_year(1970), 0);
        assert_eq!(ts_at_start_of_year(2000), 946_684_800);
        assert_eq!(ts_at_start_of_year(1969), -31_536_000);
    }
}
