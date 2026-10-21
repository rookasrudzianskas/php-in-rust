//! Helpers shared by the strtotime and parse-from-format parsers (`parse_date.re`).
//!
//! The C helpers walk NUL-terminated strings; [`Cur`] emulates that by returning 0 for
//! any position at or past the end of its slice.

use std::sync::Arc;

use crate::error::{ErrorContainer, code};
use crate::time::{Time, UNSET, ZoneType};
use crate::tz::TzInfo;
use crate::tz::abbr::abbr_search;
use crate::tz::db::strcasecmp;

/// Callback used to resolve timezone identifiers (`timelib_tz_get_wrapper`).
pub type TzGetter<'a> = &'a dyn Fn(&str) -> Option<Arc<TzInfo>>;

/// Maximum abbreviation length + 1 (`MAX_ABBR_LEN`, `_POSIX_TZNAME_MAX` = 6).
const MAX_ABBR_LEN: usize = 6;

/// A cursor over a NUL-terminated byte string.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cur<'a> {
    pub(crate) s: &'a [u8],
    pub(crate) p: usize,
}

impl<'a> Cur<'a> {
    /// Creates a cursor over `s`, truncated at its first NUL (C string semantics).
    pub(crate) fn new(s: &'a [u8]) -> Self {
        let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
        Cur { s: &s[..end], p: 0 }
    }

    pub(crate) fn c(&self) -> u8 {
        self.at(0)
    }

    pub(crate) fn at(&self, k: usize) -> u8 {
        self.s.get(self.p + k).copied().unwrap_or(0)
    }

    pub(crate) fn bump(&mut self) {
        self.p += 1;
    }
}

/// `strtoll`-like parse of leading decimal digits (saturating; the callers limit the
/// number of digits).
fn parse_digits(d: &[u8]) -> i64 {
    let mut v: i64 = 0;
    for &b in d.iter().take_while(|b| b.is_ascii_digit()) {
        v = v.saturating_mul(10).saturating_add(i64::from(b - b'0'));
    }
    v
}

/// `strtol(p, NULL, 10)` on the bytes starting at `p` (digits only, no sign).
fn strtol_at(c: &Cur, k: usize) -> i64 {
    parse_digits(c.s.get(c.p + k..).unwrap_or(&[]))
}

/// `timelib_get_nr_ex`: skips non-digits, reads up to `max_length` digits. Returns
/// `(value, scanned_length)`; value is [`UNSET`] (and length 0) at the end of input.
pub(crate) fn get_nr_ex(c: &mut Cur, max_length: usize) -> (i64, usize) {
    while !c.c().is_ascii_digit() {
        if c.c() == 0 {
            return (UNSET, 0);
        }
        c.bump();
    }
    let begin = c.p;
    let mut len = 0;
    while c.c().is_ascii_digit() && len < max_length {
        c.bump();
        len += 1;
    }
    (parse_digits(&c.s[begin..c.p]), c.p - begin)
}

/// `timelib_get_nr`.
pub(crate) fn get_nr(c: &mut Cur, max_length: usize) -> i64 {
    get_nr_ex(c, max_length).0
}

/// `timelib_skip_day_suffix`.
pub(crate) fn skip_day_suffix(c: &mut Cur) {
    if is_c_space(c.c()) {
        return;
    }
    let two = [c.at(0).to_ascii_lowercase(), c.at(1).to_ascii_lowercase()];
    if matches!(&two, b"nd" | b"rd" | b"st" | b"th") {
        c.p += 2;
    }
}

/// C `isspace` in the "C" locale.
pub(crate) fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `timelib_get_frac_nr`: reads `[.:][0-9]*` style fractions as microseconds.
pub(crate) fn get_frac_nr(c: &mut Cur) -> i64 {
    while c.c() != b'.' && c.c() != b':' && !c.c().is_ascii_digit() {
        if c.c() == 0 {
            return UNSET;
        }
        c.bump();
    }
    let begin = c.p;
    while c.c() == b'.' || c.c() == b':' || c.c().is_ascii_digit() {
        c.bump();
    }
    let end = c.p;
    let body = &c.s[begin + 1..end];
    let digits_end = body
        .iter()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(body.len());
    let value: f64 = std::str::from_utf8(&body[..digits_end])
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    (value * 10f64.powf(7.0 - (end - begin) as f64)) as i64
}

/// Outcome of [`get_signed_nr`].
pub(crate) enum SignedNr {
    /// A number.
    Ok(i64),
    /// `TIMELIB_ERR_UNEXPECTED_DATA` ("Found unexpected data").
    UnexpectedData,
    /// `TIMELIB_ERR_NUMBER_OUT_OF_RANGE` ("Number out of range").
    OutOfRange,
}

/// `timelib_get_signed_nr`: optional (repeated) signs followed by up to `max_length`
/// digits.
pub(crate) fn get_signed_nr(c: &mut Cur, max_length: usize) -> SignedNr {
    while !c.c().is_ascii_digit() && c.c() != b'+' && c.c() != b'-' {
        if c.c() == 0 {
            return SignedNr::UnexpectedData;
        }
        c.bump();
    }
    let mut negative = false;
    while c.c() == b'+' || c.c() == b'-' {
        if c.c() == b'-' {
            negative = !negative;
        }
        c.bump();
    }
    while !c.c().is_ascii_digit() {
        if c.c() == 0 {
            return SignedNr::UnexpectedData;
        }
        c.bump();
    }
    let begin = c.p;
    let mut len = 0;
    while c.c().is_ascii_digit() && len < max_length {
        c.bump();
        len += 1;
    }
    // strtoll with ERANGE detection
    let mut v: i128 = 0;
    for &b in &c.s[begin..c.p] {
        v = v * 10 + i128::from(b - b'0');
        if v > i128::from(i64::MAX) + 1 {
            return SignedNr::OutOfRange;
        }
    }
    if negative {
        v = -v;
    }
    match i64::try_from(v) {
        Ok(v) => SignedNr::Ok(v),
        Err(_) => SignedNr::OutOfRange,
    }
}

/// A relative unit (`timelib_relunit`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct RelUnit {
    pub(crate) name: &'static str,
    pub(crate) unit: Unit,
    pub(crate) multiplier: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Second,
    Minute,
    Hour,
    Day,
    Month,
    Year,
    Weekday,
    Special,
    Microsec,
}

macro_rules! ru {
    ($n:expr, $u:ident, $m:expr) => {
        RelUnit {
            name: $n,
            unit: Unit::$u,
            multiplier: $m,
        }
    };
}

static RELUNIT_LOOKUP: &[RelUnit] = &[
    ru!("ms", Microsec, 1000),
    ru!("msec", Microsec, 1000),
    ru!("msecs", Microsec, 1000),
    ru!("millisecond", Microsec, 1000),
    ru!("milliseconds", Microsec, 1000),
    ru!("\u{b5}s", Microsec, 1),
    ru!("usec", Microsec, 1),
    ru!("usecs", Microsec, 1),
    ru!("\u{b5}sec", Microsec, 1),
    ru!("\u{b5}secs", Microsec, 1),
    ru!("microsecond", Microsec, 1),
    ru!("microseconds", Microsec, 1),
    ru!("sec", Second, 1),
    ru!("secs", Second, 1),
    ru!("second", Second, 1),
    ru!("seconds", Second, 1),
    ru!("min", Minute, 1),
    ru!("mins", Minute, 1),
    ru!("minute", Minute, 1),
    ru!("minutes", Minute, 1),
    ru!("hour", Hour, 1),
    ru!("hours", Hour, 1),
    ru!("day", Day, 1),
    ru!("days", Day, 1),
    ru!("week", Day, 7),
    ru!("weeks", Day, 7),
    ru!("fortnight", Day, 14),
    ru!("fortnights", Day, 14),
    ru!("forthnight", Day, 14),
    ru!("forthnights", Day, 14),
    ru!("month", Month, 1),
    ru!("months", Month, 1),
    ru!("year", Year, 1),
    ru!("years", Year, 1),
    ru!("mondays", Weekday, 1),
    ru!("monday", Weekday, 1),
    ru!("mon", Weekday, 1),
    ru!("tuesdays", Weekday, 2),
    ru!("tuesday", Weekday, 2),
    ru!("tue", Weekday, 2),
    ru!("wednesdays", Weekday, 3),
    ru!("wednesday", Weekday, 3),
    ru!("wed", Weekday, 3),
    ru!("thursdays", Weekday, 4),
    ru!("thursday", Weekday, 4),
    ru!("thu", Weekday, 4),
    ru!("fridays", Weekday, 5),
    ru!("friday", Weekday, 5),
    ru!("fri", Weekday, 5),
    ru!("saturdays", Weekday, 6),
    ru!("saturday", Weekday, 6),
    ru!("sat", Weekday, 6),
    ru!("sundays", Weekday, 0),
    ru!("sunday", Weekday, 0),
    ru!("sun", Weekday, 0),
    ru!("weekday", Special, 1),
    ru!("weekdays", Special, 1),
];

/// `timelib_reltext_lookup`: (name, behavior, value).
static RELTEXT_LOOKUP: &[(&str, i32, i64)] = &[
    ("first", 0, 1),
    ("next", 0, 1),
    ("second", 0, 2),
    ("third", 0, 3),
    ("fourth", 0, 4),
    ("fifth", 0, 5),
    ("sixth", 0, 6),
    ("seventh", 0, 7),
    ("eight", 0, 8),
    ("eighth", 0, 8),
    ("ninth", 0, 9),
    ("tenth", 0, 10),
    ("eleventh", 0, 11),
    ("twelfth", 0, 12),
    ("last", 0, -1),
    ("previous", 0, -1),
    ("this", 1, 0),
];

/// `timelib_month_lookup`.
static MONTH_LOOKUP: &[(&str, i64)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("sept", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
    ("i", 1),
    ("ii", 2),
    ("iii", 3),
    ("iv", 4),
    ("v", 5),
    ("vi", 6),
    ("vii", 7),
    ("viii", 8),
    ("ix", 9),
    ("x", 10),
    ("xi", 11),
    ("xii", 12),
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
];

fn read_alpha<'a>(c: &mut Cur<'a>) -> &'a [u8] {
    let begin = c.p;
    while c.c().is_ascii_alphabetic() {
        c.bump();
    }
    &c.s[begin..c.p]
}

/// `timelib_lookup_relative_text`: returns the value and, if the word was known, its
/// behavior.
pub(crate) fn lookup_relative_text(c: &mut Cur) -> (i64, Option<i32>) {
    let word = read_alpha(c);
    let mut value = 0;
    let mut behavior = None;
    for &(name, b, v) in RELTEXT_LOOKUP {
        if strcasecmp(word, name.as_bytes()).is_eq() {
            value = v;
            behavior = Some(b);
        }
    }
    (value, behavior)
}

/// `timelib_get_relative_text`.
pub(crate) fn get_relative_text(c: &mut Cur) -> (i64, Option<i32>) {
    while matches!(c.c(), b' ' | b'\t' | b'-' | b'/') {
        c.bump();
    }
    lookup_relative_text(c)
}

/// `timelib_lookup_month`: 0 if not a month name.
pub(crate) fn lookup_month(c: &mut Cur) -> i64 {
    let word = read_alpha(c);
    let mut value = 0;
    for &(name, v) in MONTH_LOOKUP {
        if strcasecmp(word, name.as_bytes()).is_eq() {
            value = v;
        }
    }
    value
}

/// `timelib_get_month`.
pub(crate) fn get_month(c: &mut Cur) -> i64 {
    while matches!(c.c(), b' ' | b'\t' | b'-' | b'.' | b'/') {
        c.bump();
    }
    lookup_month(c)
}

/// `timelib_eat_spaces`: spaces, tabs, NBSP and NNBSP.
pub(crate) fn eat_spaces(c: &mut Cur) {
    loop {
        if c.c() == b' ' || c.c() == b'\t' {
            c.bump();
        } else if c.at(0) == 0xe2 && c.at(1) == 0x80 && c.at(2) == 0xaf {
            c.p += 3;
        } else if c.at(0) == 0xc2 && c.at(1) == 0xa0 {
            c.p += 2;
        } else {
            break;
        }
    }
}

/// `timelib_eat_until_separator`.
pub(crate) fn eat_until_separator(c: &mut Cur) {
    c.bump();
    while c.c() != 0 && !b" \t.,:;/-0123456789".contains(&c.c()) {
        c.bump();
    }
}

/// `timelib_lookup_relunit`.
pub(crate) fn lookup_relunit(c: &mut Cur) -> Option<&'static RelUnit> {
    let begin = c.p;
    while !matches!(
        c.c(),
        0 | b' ' | b',' | b'\t' | b';' | b':' | b'/' | b'.' | b'-' | b'(' | b')'
    ) {
        c.bump();
    }
    let word = &c.s[begin..c.p];
    RELUNIT_LOOKUP
        .iter()
        .find(|tp| strcasecmp(word, tp.name.as_bytes()).is_eq())
}

/// `timelib_meridian`: returns the hour correction (-12, 0 or +12).
pub(crate) fn meridian(c: &mut Cur, h: i64) -> i64 {
    while c.c() != 0 && !b"AaPp".contains(&c.c()) {
        c.bump();
    }
    let mut retval = 0;
    if c.c() == b'a' || c.c() == b'A' {
        if h == 12 {
            retval = -12;
        }
    } else if h != 12 {
        retval = 12;
    }
    c.bump();
    if c.c() == b'.' {
        c.bump();
    }
    if c.c() == b'M' || c.c() == b'm' {
        c.bump();
    }
    if c.c() == b'.' {
        c.bump();
    }
    retval
}

/// `timelib_meridian_with_check`: like [`meridian`] but returns [`UNSET`] if no valid
/// meridian is present.
pub(crate) fn meridian_with_check(c: &mut Cur, h: i64) -> i64 {
    while c.c() != 0 && !b"AaPp".contains(&c.c()) {
        c.bump();
    }
    if c.c() == 0 {
        return UNSET;
    }
    let mut retval = 0;
    if c.c() == b'a' || c.c() == b'A' {
        if h == 12 {
            retval = -12;
        }
    } else if h != 12 {
        retval = 12;
    }
    c.bump();
    if c.c() == b'.' {
        c.bump();
        if c.c() != b'm' && c.c() != b'M' {
            return UNSET;
        }
        c.bump();
        if c.c() != b'.' {
            return UNSET;
        }
        c.bump();
    } else if c.c() == b'm' || c.c() == b'M' {
        c.bump();
    } else {
        return UNSET;
    }
    retval
}

/// `PROCESS_YEAR`: two digit years map to 1970..2069.
pub(crate) fn process_year(y: &mut i64, length: usize) {
    if *y == UNSET || length >= 4 {
        return;
    }
    if *y < 100 {
        if *y < 70 {
            *y += 2000;
        } else {
            *y += 1900;
        }
    }
}

fn s_hour(a: i64) -> i64 {
    i64::from(a.wrapping_mul(3600) as i32)
}

fn s_min(a: i64) -> i64 {
    i64::from(a.wrapping_mul(60) as i32)
}

/// `timelib_parse_tz_cor`: parses `H`, `HH`, `H:M`, `HHMM`, `HH:MM`, `HHMMSS`,
/// `HH:MM:SS`. Returns `(seconds, found)`.
pub(crate) fn parse_tz_cor(c: &mut Cur) -> (i64, bool) {
    let begin = *c;
    while c.c().is_ascii_digit() || c.c() == b':' {
        c.bump();
    }
    let len = c.p - begin.p;
    let b = |k: usize| begin.at(k);
    match len {
        1 | 2 => (s_hour(strtol_at(&begin, 0)), true),
        3 | 4 => {
            if b(1) == b':' {
                (
                    s_hour(strtol_at(&begin, 0)) + s_min(strtol_at(&begin, 2)),
                    true,
                )
            } else if b(2) == b':' {
                (
                    s_hour(strtol_at(&begin, 0)) + s_min(strtol_at(&begin, 3)),
                    true,
                )
            } else {
                let tmp = strtol_at(&begin, 0);
                (s_hour(tmp / 100) + s_min(tmp % 100), true)
            }
        }
        5 => {
            if b(2) != b':' {
                return (0, false);
            }
            (
                s_hour(strtol_at(&begin, 0)) + s_min(strtol_at(&begin, 3)),
                true,
            )
        }
        6 => {
            let tmp = strtol_at(&begin, 0);
            (
                s_hour(tmp / 10000) + s_min((tmp / 100) % 100) + (tmp % 100),
                true,
            )
        }
        8 => {
            if b(2) != b':' || b(5) != b':' {
                return (0, false);
            }
            (
                s_hour(strtol_at(&begin, 0)) + s_min(strtol_at(&begin, 3)) + strtol_at(&begin, 6),
                true,
            )
        }
        _ => (0, false),
    }
}

/// `timelib_parse_tz_minutes`: `[+-]minutes`.
pub(crate) fn parse_tz_minutes(c: &mut Cur, t: &mut Time) -> i64 {
    let begin = *c;
    if c.c() != b'+' && c.c() != b'-' {
        return UNSET;
    }
    c.bump();
    while c.c().is_ascii_digit() {
        c.bump();
    }
    t.is_localtime = true;
    t.zone_type = ZoneType::Offset;
    t.dst = 0;
    let v = s_min(strtol_at(&begin, 1));
    if begin.c() == b'+' { v } else { -v }
}

/// `timelib_lookup_abbr`: returns `(offset, dst, word, found)`.
fn lookup_abbr<'a>(c: &mut Cur<'a>) -> (i64, Option<i32>, &'a [u8]) {
    let begin = c.p;
    while c.c().is_ascii_alphanumeric() || matches!(c.c(), b'/' | b'_' | b'-' | b'+') {
        c.bump();
    }
    let word = &c.s[begin..c.p];
    if word.len() < MAX_ABBR_LEN
        && let Some(tp) = abbr_search(word, -1, 0)
    {
        let dst = i32::from(tp.dst);
        let value = i64::from(tp.gmtoffset) - i64::from(dst) * 3600;
        return (value, Some(dst), word);
    }
    (0, None, word)
}

/// `timelib_parse_zone`: parses an offset, abbreviation or identifier into `t`. Returns
/// `(offset, tz_not_found)`; the offset is meant for `t.z`.
pub(crate) fn parse_zone(c: &mut Cur, t: &mut Time, tz_get: TzGetter) -> (i64, bool) {
    let mut paren_count = 0usize;
    let tz_not_found;
    while matches!(c.c(), b' ' | b'\t' | b'(') {
        if c.c() == b'(' {
            paren_count += 1;
        }
        c.bump();
    }
    if c.at(0) == b'G' && c.at(1) == b'M' && c.at(2) == b'T' && matches!(c.at(3), b'+' | b'-') {
        c.p += 3;
    }
    let retval = if c.c() == b'+' || c.c() == b'-' {
        let negative = c.c() == b'-';
        c.bump();
        t.is_localtime = true;
        t.zone_type = ZoneType::Offset;
        t.dst = 0;
        let (v, found) = parse_tz_cor(c);
        tz_not_found = !found;
        if negative { -v } else { v }
    } else {
        t.is_localtime = true;
        let (offset, dst, word) = lookup_abbr(c);
        let mut found = 0;
        if let Some(dst) = dst {
            found = 1;
            t.dst = dst;
            t.zone_type = ZoneType::Abbr;
            t.tz_abbr_update(&String::from_utf8_lossy(word));
        }
        if found == 0 || word == b"UTC" {
            let name = String::from_utf8_lossy(word);
            if let Some(res) = tz_get(&name) {
                t.tz_info = Some(res);
                t.zone_type = ZoneType::Id;
                found += 1;
            }
        }
        tz_not_found = found == 0;
        offset
    };
    while paren_count > 0 && c.c() == b')' {
        c.bump();
        paren_count -= 1;
    }
    (retval, tz_not_found)
}

/// Adds `amount * multiplier` to `e`, reporting overflow (`add_with_overflow`).
pub(crate) fn add_with_overflow(
    e: &mut i64,
    amount: i64,
    multiplier: i32,
    errors: &mut ErrorContainer,
    pos: usize,
    ch: u8,
) {
    let (r, overflow) = e.overflowing_add(amount.wrapping_mul(i64::from(multiplier)));
    *e = r;
    if overflow {
        errors.add_error(
            code::ERR_NUMBER_OUT_OF_RANGE,
            pos,
            ch,
            "Number out of range",
        );
    }
}
