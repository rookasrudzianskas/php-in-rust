//! The strtotime parser (`timelib_strtotime` in `parse_date.re`).

use super::common::{
    Cur, SignedNr, TzGetter, Unit, add_with_overflow, eat_spaces, get_frac_nr, get_month, get_nr,
    get_nr_ex, get_relative_text, get_signed_nr, is_c_space, lookup_month, lookup_relunit,
    meridian, parse_zone, process_year, skip_day_suffix,
};
use super::grammar::{Rule, next_token};
use crate::dow::{daynr_from_weeknr, valid_date, valid_time};
use crate::error::{ErrorContainer, code};
use crate::time::{
    SPECIAL_DAY_OF_WEEK_IN_MONTH, SPECIAL_FIRST_DAY_OF_MONTH, SPECIAL_LAST_DAY_OF_MONTH,
    SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH, SPECIAL_WEEKDAY, Time, UNSET, ZoneType,
};

/// Padding appended to the input, like re2c's `YYMAXFILL`.
const YYMAXFILL: usize = 36;

/// Result of a parse: the (partially filled) time and the collected errors/warnings.
#[derive(Clone, Debug)]
pub struct ParseResult {
    /// The parsed time; unset fields are [`UNSET`].
    pub time: Time,
    /// Errors and warnings.
    pub errors: ErrorContainer,
}

struct Scanner<'a> {
    buf: Vec<u8>,
    tok: usize,
    errors: ErrorContainer,
    time: Time,
    tz_get: TzGetter<'a>,
}

/// Whether to keep the time portion in [`Scanner::set_relative`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum TimePart {
    DontKeep,
    Keep,
}

impl Scanner<'_> {
    fn tok_char(&self) -> u8 {
        self.buf.get(self.tok).copied().unwrap_or(0)
    }

    fn add_error(&mut self, code: i32, msg: &'static str) {
        let ch = self.tok_char();
        self.errors.add_error(code, self.tok, ch, msg);
    }

    fn add_warning(&mut self, code: i32, msg: &'static str) {
        let ch = self.tok_char();
        self.errors.add_warning(code, self.tok, ch, msg);
    }

    // The TIMELIB_* macros. `None` means "return TIMELIB_ERROR from the action".

    fn have_time(&mut self) -> Option<()> {
        if self.time.have_time != 0 {
            self.add_error(code::ERR_DOUBLE_TIME, "Double time specification");
            return None;
        }
        self.time.have_time = 1;
        self.time.h = 0;
        self.time.i = 0;
        self.time.s = 0;
        self.time.us = 0;
        Some(())
    }

    fn unhave_time(&mut self) {
        self.time.have_time = 0;
        self.time.h = 0;
        self.time.i = 0;
        self.time.s = 0;
        self.time.us = 0;
    }

    fn have_date(&mut self) -> Option<()> {
        if self.time.have_date != 0 {
            self.add_error(code::ERR_DOUBLE_DATE, "Double date specification");
            return None;
        }
        self.time.have_date = 1;
        Some(())
    }

    fn unhave_date(&mut self) {
        self.time.have_date = 0;
        self.time.d = 0;
        self.time.m = 0;
        self.time.y = 0;
    }

    fn have_relative(&mut self) {
        self.time.have_relative = true;
    }

    fn have_weekday_relative(&mut self) {
        self.time.have_relative = true;
        self.time.relative.have_weekday_relative = true;
    }

    fn have_special_relative(&mut self) {
        self.time.have_relative = true;
        self.time.relative.have_special_relative = true;
    }

    fn have_tz(&mut self) -> Option<()> {
        if self.time.have_zone != 0 {
            if self.time.have_zone > 1 {
                self.add_error(code::ERR_DOUBLE_TZ, "Double timezone specification");
            } else {
                self.add_warning(code::WARN_DOUBLE_TZ, "Double timezone specification");
            }
            self.time.have_zone += 1;
            return None;
        }
        self.time.have_zone += 1;
        Some(())
    }

    fn signed_nr(&mut self, c: &mut Cur, max_length: usize) -> i64 {
        match get_signed_nr(c, max_length) {
            SignedNr::Ok(v) => v,
            SignedNr::UnexpectedData => {
                self.add_error(code::ERR_UNEXPECTED_DATA, "Found unexpected data");
                0
            }
            SignedNr::OutOfRange => {
                self.add_error(code::ERR_NUMBER_OUT_OF_RANGE, "Number out of range");
                0
            }
        }
    }

    fn zone(&mut self, c: &mut Cur) {
        let (z, not_found) = parse_zone(c, &mut self.time, self.tz_get);
        self.time.z = z as i32;
        if not_found {
            self.add_error(
                code::ERR_TZID_NOT_FOUND,
                "The timezone could not be found in the database",
            );
        }
    }

    /// `timelib_set_relative`.
    fn set_relative(&mut self, c: &mut Cur, amount: i64, behavior: i32, time_part: TimePart) {
        let Some(relunit) = lookup_relunit(c) else {
            return;
        };
        let (pos, ch) = (self.tok, self.tok_char());
        let rel = &mut self.time.relative;
        let m = relunit.multiplier;
        match relunit.unit {
            Unit::Microsec => add_with_overflow(&mut rel.us, amount, m, &mut self.errors, pos, ch),
            Unit::Second => add_with_overflow(&mut rel.s, amount, m, &mut self.errors, pos, ch),
            Unit::Minute => add_with_overflow(&mut rel.i, amount, m, &mut self.errors, pos, ch),
            Unit::Hour => add_with_overflow(&mut rel.h, amount, m, &mut self.errors, pos, ch),
            Unit::Day => add_with_overflow(&mut rel.d, amount, m, &mut self.errors, pos, ch),
            Unit::Month => add_with_overflow(&mut rel.m, amount, m, &mut self.errors, pos, ch),
            Unit::Year => add_with_overflow(&mut rel.y, amount, m, &mut self.errors, pos, ch),
            Unit::Weekday => {
                self.have_weekday_relative();
                if time_part != TimePart::Keep {
                    self.unhave_time();
                }
                let rel = &mut self.time.relative;
                let weeks = if amount > 0 { amount - 1 } else { amount };
                rel.d = rel.d.wrapping_add(weeks.wrapping_mul(7));
                rel.weekday = m;
                rel.weekday_behavior = behavior;
            }
            Unit::Special => {
                self.have_special_relative();
                if time_part != TimePart::Keep {
                    self.unhave_time();
                }
                self.time.relative.special.kind = m as u32;
                self.time.relative.special.amount = amount;
            }
        }
    }

    fn run(&mut self, rule: Rule, token: &[u8]) {
        let mut c = Cur::new(token);
        // Actions return `None` when they bail out with TIMELIB_ERROR.
        let _ = self.action(rule, &mut c);
    }

    #[allow(clippy::too_many_lines)]
    fn action(&mut self, rule: Rule, c: &mut Cur) -> Option<()> {
        match rule {
            Rule::Yesterday => {
                self.have_relative();
                self.unhave_time();
                self.time.relative.d = -1;
            }
            Rule::Now => {}
            Rule::Noon => {
                self.unhave_time();
                self.have_time()?;
                self.time.h = 12;
            }
            Rule::MidnightToday => self.unhave_time(),
            Rule::Tomorrow => {
                self.have_relative();
                self.unhave_time();
                self.time.relative.d = 1;
            }
            Rule::Timestamp | Rule::TimestampMs => {
                self.have_relative();
                self.unhave_date();
                self.unhave_time();
                self.have_tz()?;
                let is_negative = c.at(1) == b'-';
                let i = self.signed_nr(c, 24);
                let mut us = 0i64;
                if rule == Rule::TimestampMs {
                    let before = c.p;
                    let raw = self.signed_nr(c, 6) as u64;
                    let scaled = (raw as f64 * 10f64.powf(7.0 - (c.p - before) as f64)) as u64;
                    us = scaled as i64;
                    if is_negative {
                        us = us.wrapping_neg();
                    }
                }
                let t = &mut self.time;
                t.y = 1970;
                t.m = 1;
                t.d = 1;
                t.h = 0;
                t.i = 0;
                t.s = 0;
                t.us = 0;
                t.relative.s = t.relative.s.wrapping_add(i);
                if rule == Rule::TimestampMs {
                    t.relative.us = us;
                }
                t.is_localtime = true;
                t.zone_type = ZoneType::Offset;
                t.z = 0;
                t.dst = 0;
            }
            Rule::FirstLastDayOf => {
                self.have_relative();
                self.time.relative.first_last_day_of = if matches!(c.c(), b'l' | b'L') {
                    SPECIAL_LAST_DAY_OF_MONTH
                } else {
                    SPECIAL_FIRST_DAY_OF_MONTH
                };
            }
            Rule::BackFrontOf => {
                self.unhave_time();
                self.have_time()?;
                if c.c() == b'b' {
                    self.time.h = get_nr(c, 2);
                    self.time.i = 15;
                } else {
                    self.time.h = get_nr(c, 2) - 1;
                    self.time.i = 45;
                }
                if c.c() != 0 {
                    eat_spaces(c);
                    self.time.h += meridian(c, self.time.h);
                }
            }
            Rule::WeekdayOf => {
                self.have_relative();
                self.have_special_relative();
                let (i, behavior) = get_relative_text(c);
                let behavior = behavior.unwrap_or(0);
                eat_spaces(c);
                if i > 0 {
                    self.time.relative.special.kind = SPECIAL_DAY_OF_WEEK_IN_MONTH;
                    self.set_relative(c, i, 1, TimePart::DontKeep);
                } else {
                    self.time.relative.special.kind = SPECIAL_LAST_DAY_OF_WEEK_IN_MONTH;
                    self.set_relative(c, i, behavior, TimePart::DontKeep);
                }
            }
            Rule::Time12 => {
                self.have_time()?;
                self.time.h = get_nr(c, 2);
                if matches!(c.c(), b':' | b'.') {
                    self.time.i = get_nr(c, 2);
                    if matches!(c.c(), b':' | b'.') {
                        self.time.s = get_nr(c, 2);
                    }
                }
                eat_spaces(c);
                self.time.h += meridian(c, self.time.h);
            }
            Rule::MssqlTime => {
                self.have_time()?;
                self.time.h = get_nr(c, 2);
                self.time.i = get_nr(c, 2);
                if matches!(c.c(), b':' | b'.') {
                    self.time.s = get_nr(c, 2);
                    if matches!(c.c(), b':' | b'.') {
                        self.time.us = get_frac_nr(c);
                    }
                }
                eat_spaces(c);
                self.time.h += meridian(c, self.time.h);
            }
            Rule::Time24 => {
                self.have_time()?;
                self.time.h = get_nr(c, 2);
                if matches!(c.c(), b':' | b'.') {
                    self.time.i = get_nr(c, 2);
                    if matches!(c.c(), b':' | b'.') {
                        self.time.s = get_nr(c, 2);
                        if c.c() == b'.' {
                            self.time.us = get_frac_nr(c);
                        }
                    }
                }
                if c.c() != 0 {
                    self.zone(c);
                }
            }
            Rule::GnuNoColon => {
                match self.time.have_time {
                    0 => {
                        self.time.h = get_nr(c, 2);
                        self.time.i = get_nr(c, 2);
                        self.time.s = 0;
                    }
                    1 => self.time.y = get_nr(c, 4),
                    _ => {
                        self.add_error(code::ERR_DOUBLE_TIME, "Double time specification");
                        return None;
                    }
                }
                self.time.have_time += 1;
            }
            Rule::Iso8601NoColon => {
                self.have_time()?;
                self.time.h = get_nr(c, 2);
                self.time.i = get_nr(c, 2);
                self.time.s = get_nr(c, 2);
                if c.c() != 0 {
                    self.zone(c);
                }
            }
            Rule::American => {
                self.have_date()?;
                self.time.m = get_nr(c, 2);
                self.time.d = get_nr(c, 2);
                if c.c() == b'/' {
                    let (y, len) = get_nr_ex(c, 4);
                    self.time.y = y;
                    process_year(&mut self.time.y, len);
                }
            }
            Rule::IsoDate4 => {
                self.have_date()?;
                self.time.y = self.signed_nr(c, 4);
                self.time.m = get_nr(c, 2);
                self.time.d = get_nr(c, 2);
            }
            Rule::IsoDate2 => {
                self.have_date()?;
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                self.time.m = get_nr(c, 2);
                self.time.d = get_nr(c, 2);
                process_year(&mut self.time.y, len);
            }
            Rule::IsoDateX => {
                self.have_date()?;
                self.time.y = self.signed_nr(c, 19);
                self.time.m = get_nr(c, 2);
                self.time.d = get_nr(c, 2);
            }
            Rule::GnuDateShorter => {
                self.have_date()?;
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                self.time.m = get_nr(c, 2);
                self.time.d = 1;
                process_year(&mut self.time.y, len);
            }
            Rule::GnuDateShort => {
                self.have_date()?;
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                self.time.m = get_nr(c, 2);
                self.time.d = get_nr(c, 2);
                process_year(&mut self.time.y, len);
            }
            Rule::DateFull => {
                self.have_date()?;
                self.time.d = get_nr(c, 2);
                skip_day_suffix(c);
                self.time.m = get_month(c);
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                process_year(&mut self.time.y, len);
            }
            Rule::PointedDate4 => {
                self.have_date()?;
                self.time.d = get_nr(c, 2);
                self.time.m = get_nr(c, 2);
                self.time.y = get_nr(c, 4);
            }
            Rule::PointedDate2 => {
                self.have_date()?;
                self.time.d = get_nr(c, 2);
                self.time.m = get_nr(c, 2);
                let (y, len) = get_nr_ex(c, 2);
                self.time.y = y;
                process_year(&mut self.time.y, len);
            }
            Rule::DateNoDay => {
                self.have_date()?;
                self.time.m = get_month(c);
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                self.time.d = 1;
                process_year(&mut self.time.y, len);
            }
            Rule::DateNoDayRev => {
                self.have_date()?;
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                self.time.m = get_month(c);
                self.time.d = 1;
                process_year(&mut self.time.y, len);
            }
            Rule::DateTextual | Rule::PgTextShort => {
                self.have_date()?;
                self.time.m = get_month(c);
                self.time.d = get_nr(c, 2);
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                process_year(&mut self.time.y, len);
            }
            Rule::DateNoYearRev => {
                self.have_date()?;
                self.time.d = get_nr(c, 2);
                skip_day_suffix(c);
                self.time.m = get_month(c);
            }
            Rule::DateNoColon => {
                self.have_date()?;
                self.time.y = get_nr(c, 4);
                self.time.m = get_nr(c, 2);
                self.time.d = get_nr(c, 2);
            }
            Rule::XmlRpc => {
                self.have_time()?;
                self.have_date()?;
                self.time.y = get_nr(c, 4);
                self.time.m = get_nr(c, 2);
                self.time.d = get_nr(c, 2);
                self.time.h = get_nr(c, 2);
                self.time.i = get_nr(c, 2);
                self.time.s = get_nr(c, 2);
                if c.c() == b'.' {
                    self.time.us = get_frac_nr(c);
                    if c.c() != 0 {
                        self.zone(c);
                    }
                }
            }
            Rule::PgYdotd => {
                self.have_date()?;
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                self.time.d = get_nr(c, 3);
                self.time.m = 1;
                process_year(&mut self.time.y, len);
            }
            Rule::IsoWeekDay | Rule::IsoWeek => {
                self.have_date()?;
                self.have_relative();
                self.time.y = get_nr(c, 4);
                let w = get_nr(c, 2);
                let d = if rule == Rule::IsoWeekDay {
                    get_nr(c, 1)
                } else {
                    1
                };
                self.time.m = 1;
                self.time.d = 1;
                self.time.relative.d = daynr_from_weeknr(self.time.y, w, d);
            }
            Rule::PgTextReverse => {
                self.have_date()?;
                let (y, len) = get_nr_ex(c, 4);
                self.time.y = y;
                self.time.m = get_month(c);
                self.time.d = get_nr(c, 2);
                process_year(&mut self.time.y, len);
            }
            Rule::Clf => {
                self.have_time()?;
                self.have_date()?;
                self.time.d = get_nr(c, 2);
                self.time.m = get_month(c);
                self.time.y = get_nr(c, 4);
                self.time.h = get_nr(c, 2);
                self.time.i = get_nr(c, 2);
                self.time.s = get_nr(c, 2);
                eat_spaces(c);
                self.zone(c);
            }
            Rule::Year4 => self.time.y = get_nr(c, 4),
            Rule::Ago => {
                let r = &mut self.time.relative;
                r.y = r.y.wrapping_neg();
                r.m = r.m.wrapping_neg();
                r.d = r.d.wrapping_neg();
                r.h = r.h.wrapping_neg();
                r.i = r.i.wrapping_neg();
                r.s = r.s.wrapping_neg();
                r.weekday = r.weekday.wrapping_neg();
                if r.weekday == 0 {
                    r.weekday = -7;
                }
                if r.have_special_relative && r.special.kind == SPECIAL_WEEKDAY {
                    r.special.amount = r.special.amount.wrapping_neg();
                }
            }
            Rule::DayText => {
                self.have_relative();
                self.have_weekday_relative();
                self.unhave_time();
                if let Some(relunit) = lookup_relunit(c) {
                    self.time.relative.weekday = relunit.multiplier;
                }
                if self.time.relative.weekday_behavior != 2 {
                    self.time.relative.weekday_behavior = 1;
                }
            }
            Rule::RelativeTextWeek => {
                self.have_relative();
                let mut behavior = 0;
                while c.c() != 0 {
                    let start = c.p;
                    let (i, b) = get_relative_text(c);
                    if let Some(b) = b {
                        behavior = b;
                    }
                    eat_spaces(c);
                    self.set_relative(c, i, behavior, TimePart::DontKeep);
                    self.time.relative.weekday_behavior = 2;
                    // to handle the format weekday + last/this/next week
                    if !self.time.relative.have_weekday_relative {
                        self.have_weekday_relative();
                        self.time.relative.weekday = 1;
                    }
                    if c.p == start {
                        break;
                    }
                }
            }
            Rule::RelativeText => {
                self.have_relative();
                let mut behavior = 0;
                while c.c() != 0 {
                    let start = c.p;
                    let (i, b) = get_relative_text(c);
                    if let Some(b) = b {
                        behavior = b;
                    }
                    eat_spaces(c);
                    self.set_relative(c, i, behavior, TimePart::DontKeep);
                    if c.p == start {
                        break;
                    }
                }
            }
            Rule::MonthText => {
                self.have_date()?;
                self.time.m = lookup_month(c);
            }
            Rule::Tz => {
                self.have_tz()?;
                eat_spaces(c);
                self.zone(c);
            }
            Rule::DateShortWithTime12 => {
                self.have_date()?;
                self.time.m = get_month(c);
                self.time.d = get_nr(c, 2);
                self.have_time()?;
                self.time.h = get_nr(c, 2);
                self.time.i = get_nr(c, 2);
                if matches!(c.c(), b':' | b'.') {
                    self.time.s = get_nr(c, 2);
                    if c.c() == b'.' {
                        self.time.us = get_frac_nr(c);
                    }
                }
                self.time.h += meridian(c, self.time.h);
            }
            Rule::DateShortWithTime24 => {
                self.have_date()?;
                self.time.m = get_month(c);
                self.time.d = get_nr(c, 2);
                self.have_time()?;
                self.time.h = get_nr(c, 2);
                self.time.i = get_nr(c, 2);
                if c.c() == b':' {
                    self.time.s = get_nr(c, 2);
                    if c.c() == b'.' {
                        self.time.us = get_frac_nr(c);
                    }
                }
                if c.c() != 0 {
                    self.zone(c);
                }
            }
            Rule::Relative => {
                self.have_relative();
                while c.c() != 0 {
                    let start = c.p;
                    let i = self.signed_nr(c, 24);
                    eat_spaces(c);
                    self.set_relative(c, i, 1, TimePart::Keep);
                    if c.p == start {
                        break;
                    }
                }
            }
            Rule::Separator | Rule::Space | Rule::NulNewline => {}
            Rule::Any => self.add_error(code::ERR_UNEXPECTED_CHARACTER, "Unexpected character"),
        }
        Some(())
    }
}

/// Parses a date/time string with PHP's `strtotime()` grammar (`timelib_strtotime`).
///
/// `tz_get` resolves timezone identifiers (use [`crate::builtin_tzinfo`] for PHP's
/// behaviour). Fields not present in the string are left as [`UNSET`]; use
/// [`Time::fill_holes`] and [`Time::update_ts`] to obtain a timestamp.
pub fn strtotime(input: impl AsRef<[u8]>, tz_get: TzGetter) -> ParseResult {
    let input = input.as_ref();
    let mut s = 0usize;
    let mut e = input.len();
    while s < e && is_c_space(input[s]) {
        s += 1;
    }
    while e > s && is_c_space(input[e - 1]) {
        e -= 1;
    }
    if s >= e {
        let mut time = Time::new();
        let mut errors = ErrorContainer::default();
        errors.add_error(code::ERR_EMPTY_STRING, 0, 0, "Empty string");
        time.y = UNSET;
        time.d = UNSET;
        time.m = UNSET;
        time.h = UNSET;
        time.i = UNSET;
        time.s = UNSET;
        time.us = UNSET;
        time.dst = UNSET as i32;
        time.z = UNSET as i32;
        time.is_localtime = false;
        time.zone_type = ZoneType::None;
        return ParseResult { time, errors };
    }
    let len = e - s;
    let mut buf = Vec::with_capacity(len + YYMAXFILL);
    buf.extend_from_slice(&input[s..e]);
    buf.resize(len + YYMAXFILL, 0);

    let mut time = Time::new_unset();
    time.is_localtime = false;
    time.zone_type = ZoneType::None;
    time.relative.days = UNSET;

    let mut sc = Scanner {
        buf,
        tok: 0,
        errors: ErrorContainer::default(),
        time,
        tz_get,
    };

    let mut cursor = 0usize;
    loop {
        sc.tok = cursor;
        // YYFILL(36) at the start state: end of input
        if sc.buf.len() - cursor < YYMAXFILL {
            break;
        }
        let (end, rule) = next_token(&sc.buf, cursor);
        let token = sc.buf[cursor..end].to_vec();
        cursor = end;
        sc.run(rule, &token);
    }

    // do funky checking whether the parsed time was valid time
    if sc.time.have_time != 0 && !valid_time(sc.time.h, sc.time.i, sc.time.s) {
        sc.add_warning(code::WARN_INVALID_TIME, "The parsed time was invalid");
    }
    // do funky checking whether the parsed date was valid date
    if sc.time.have_date != 0 && !valid_date(sc.time.y, sc.time.m, sc.time.d) {
        sc.add_warning(code::WARN_INVALID_DATE, "The parsed date was invalid");
    }

    ParseResult {
        time: sc.time,
        errors: sc.errors,
    }
}
