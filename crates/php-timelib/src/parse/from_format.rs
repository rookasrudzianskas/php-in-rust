//! `DateTime::createFromFormat()` parsing (`timelib_parse_from_format` in
//! `parse_date.re`).

use super::common::{
    Cur, SignedNr, TzGetter, eat_spaces, eat_until_separator, get_nr, get_nr_ex, get_signed_nr,
    lookup_month, lookup_relunit, meridian_with_check, parse_tz_minutes, parse_zone, process_year,
    skip_day_suffix,
};
use super::strtotime::ParseResult;
use crate::dow::{date_from_isodate, valid_date, valid_time};
use crate::error::{ErrorContainer, code};
use crate::time::{Time, UNSET, ZoneType};

/// Format specifier meanings (`timelib_format_specifier_code`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum FormatSpecifier {
    AllowExtraCharacters,
    AnySeparator,
    DayTwoDigit,
    DayTwoDigitPadded,
    DayOfWeekIso,
    DayOfWeek,
    DayOfYear,
    DaySuffix,
    End,
    EpochSeconds,
    Escape,
    HourTwoDigit12Max,
    HourTwoDigit12MaxPadded,
    HourTwoDigit24Max,
    HourTwoDigit24MaxPadded,
    Literal,
    Meridian,
    MicrosecondSixDigit,
    MillisecondThreeDigit,
    MinuteTwoDigit,
    MonthTwoDigit,
    MonthTwoDigitPadded,
    RandomChar,
    ResetAll,
    ResetAllWhenNotSet,
    SecondTwoDigit,
    Separator,
    SkipToSeparator,
    TextualDay3Letter,
    TextualDayFull,
    TextualMonth3Letter,
    TextualMonthFull,
    TimezoneOffset,
    TimezoneOffsetMinutes,
    WeekOfYearIso,
    WeekOfYear,
    Whitespace,
    YearTwoDigit,
    YearFourDigit,
    YearExpanded,
    YearIso,
}

/// A format configuration (`timelib_format_config`): the specifier map and an optional
/// prefix character that must precede specifiers (e.g. `%`).
#[derive(Clone, Copy, Debug)]
pub struct FormatConfig<'a> {
    /// `(character, meaning)` pairs.
    pub format_map: &'a [(u8, FormatSpecifier)],
    /// Prefix character, 0 for none.
    pub prefix_char: u8,
}

use FormatSpecifier as F;

/// PHP's default map (`default_format_map`).
pub static DEFAULT_FORMAT_MAP: &[(u8, FormatSpecifier)] = &[
    (b'+', F::AllowExtraCharacters),
    (b'#', F::AnySeparator),
    (b'j', F::DayTwoDigit),
    (b'd', F::DayTwoDigitPadded),
    (b'z', F::DayOfYear),
    (b'S', F::DaySuffix),
    (b'U', F::EpochSeconds),
    (b'\\', F::Escape),
    (b'h', F::HourTwoDigit12Max),
    (b'g', F::HourTwoDigit12MaxPadded),
    (b'H', F::HourTwoDigit24Max),
    (b'G', F::HourTwoDigit24MaxPadded),
    (b'a', F::Meridian),
    (b'A', F::Meridian),
    (b'u', F::MicrosecondSixDigit),
    (b'v', F::MillisecondThreeDigit),
    (b'i', F::MinuteTwoDigit),
    (b'n', F::MonthTwoDigit),
    (b'm', F::MonthTwoDigitPadded),
    (b'?', F::RandomChar),
    (b'!', F::ResetAll),
    (b'|', F::ResetAllWhenNotSet),
    (b's', F::SecondTwoDigit),
    (b';', F::Separator),
    (b':', F::Separator),
    (b'/', F::Separator),
    (b'.', F::Separator),
    (b',', F::Separator),
    (b'-', F::Separator),
    (b'(', F::Separator),
    (b')', F::Separator),
    (b'*', F::SkipToSeparator),
    (b'D', F::TextualDay3Letter),
    (b'l', F::TextualDayFull),
    (b'M', F::TextualMonth3Letter),
    (b'F', F::TextualMonthFull),
    (b'e', F::TimezoneOffset),
    (b'P', F::TimezoneOffset),
    (b'p', F::TimezoneOffset),
    (b'T', F::TimezoneOffset),
    (b'O', F::TimezoneOffset),
    (b' ', F::Whitespace),
    (b'y', F::YearTwoDigit),
    (b'Y', F::YearFourDigit),
    (b'x', F::YearExpanded),
    (b'X', F::YearExpanded),
];

/// The default configuration (no prefix).
pub const DEFAULT_FORMAT_CONFIG: FormatConfig<'static> = FormatConfig {
    format_map: DEFAULT_FORMAT_MAP,
    prefix_char: 0,
};

fn lookup_format(input: u8, map: &[(u8, FormatSpecifier)]) -> FormatSpecifier {
    if input == 0 {
        return F::Literal;
    }
    map.iter()
        .find(|(c, _)| *c == input)
        .map_or(F::Literal, |(_, f)| *f)
}

fn reset_fields(t: &mut Time) {
    t.y = 1970;
    t.m = 1;
    t.d = 1;
    t.h = 0;
    t.i = 0;
    t.s = 0;
    t.us = 0;
    t.tz_info = None;
}

fn reset_unset_fields(t: &mut Time) {
    if t.y == UNSET {
        t.y = 1970;
    }
    if t.m == UNSET {
        t.m = 1;
    }
    if t.d == UNSET {
        t.d = 1;
    }
    if t.h == UNSET {
        t.h = 0;
    }
    if t.i == UNSET {
        t.i = 0;
    }
    if t.s == UNSET {
        t.s = 0;
    }
    if t.us == UNSET {
        t.us = 0;
    }
}

/// Parses `string` according to `format` with PHP's default specifiers
/// (`timelib_parse_from_format`).
pub fn parse_from_format(
    format: impl AsRef<[u8]>,
    string: impl AsRef<[u8]>,
    tz_get: TzGetter,
) -> ParseResult {
    parse_from_format_with_map(format, string, tz_get, &DEFAULT_FORMAT_CONFIG)
}

/// Parses `string` according to `format` with a custom specifier map
/// (`timelib_parse_from_format_with_map`).
#[allow(clippy::too_many_lines)]
pub fn parse_from_format_with_map(
    format: impl AsRef<[u8]>,
    string: impl AsRef<[u8]>,
    tz_get: TzGetter,
    config: &FormatConfig,
) -> ParseResult {
    let mut f = Cur::new(format.as_ref());
    let mut p = Cur::new(string.as_ref());
    let mut errors = ErrorContainer::default();
    let mut t = Time::new_unset();
    t.is_localtime = false;
    t.zone_type = ZoneType::None;

    let mut allow_extra = false;
    let mut prefix_found = false;
    let mut iso_year = UNSET;
    let mut iso_week_of_year = UNSET;
    let mut iso_day_of_week = UNSET;
    let prefix_char = config.prefix_char;
    let map = config.format_map;

    macro_rules! err {
        ($code:expr, $msg:expr, $at:expr) => {{
            let at: usize = $at;
            errors.add_error($code, at, p.s.get(at).copied().unwrap_or(0), $msg)
        }};
    }
    macro_rules! warn {
        ($code:expr, $msg:expr, $at:expr) => {{
            let at: usize = $at;
            errors.add_warning($code, at, p.s.get(at).copied().unwrap_or(0), $msg)
        }};
    }

    while f.c() != 0 && p.c() != 0 {
        let begin = p.p;

        if prefix_char != 0 {
            if (!prefix_found && f.c() != prefix_char) || (prefix_found && f.c() == prefix_char) {
                if f.c() != p.c() {
                    err!(
                        code::ERR_FORMAT_LITERAL_MISMATCH,
                        "Format literal not found",
                        begin
                    );
                }
                p.bump();
                f.bump();
                prefix_found = false;
                continue;
            }
            if f.c() == prefix_char {
                f.bump();
                prefix_found = true;
                continue;
            }
            prefix_found = false;
        }

        let check_number = |p: &Cur, errors: &mut ErrorContainer| {
            if !p.c().is_ascii_digit() {
                errors.add_error(
                    code::ERR_UNEXPECTED_DATA,
                    begin,
                    p.s.get(begin).copied().unwrap_or(0),
                    "Unexpected data found.",
                );
            }
        };
        let check_signed_number = |p: &Cur, errors: &mut ErrorContainer| {
            if !(p.c().is_ascii_digit() || p.c() == b'+' || p.c() == b'-') {
                errors.add_error(
                    code::ERR_UNEXPECTED_DATA,
                    begin,
                    p.s.get(begin).copied().unwrap_or(0),
                    "Unexpected data found.",
                );
            }
        };

        match lookup_format(f.c(), map) {
            F::TextualDay3Letter | F::TextualDayFull => match lookup_relunit(&mut p) {
                None => err!(
                    code::ERR_NO_TEXTUAL_DAY,
                    "A textual day could not be found",
                    begin
                ),
                Some(rel) => {
                    t.have_relative = true;
                    t.relative.have_weekday_relative = true;
                    t.relative.weekday = rel.multiplier;
                    t.relative.weekday_behavior = 1;
                }
            },
            F::DayTwoDigit | F::DayTwoDigitPadded => {
                check_number(&p, &mut errors);
                t.d = get_nr(&mut p, 2);
                if t.d == UNSET {
                    err!(
                        code::ERR_NO_TWO_DIGIT_DAY,
                        "A two digit day could not be found",
                        begin
                    );
                } else {
                    t.have_date = 1;
                }
            }
            F::DaySuffix => skip_day_suffix(&mut p),
            F::DayOfYear => {
                check_number(&p, &mut errors);
                if t.y == UNSET {
                    err!(
                        code::ERR_MERIDIAN_BEFORE_HOUR,
                        "A 'day of year' can only come after a year has been found",
                        begin
                    );
                }
                let tmp = get_nr(&mut p, 3);
                if tmp == UNSET {
                    err!(
                        code::ERR_NO_THREE_DIGIT_DAY_OF_YEAR,
                        "A three digit day-of-year could not be found",
                        begin
                    );
                } else if t.y != UNSET {
                    t.have_date = 1;
                    t.m = 1;
                    t.d = tmp + 1;
                    t.do_normalize();
                }
            }
            F::MonthTwoDigit | F::MonthTwoDigitPadded => {
                check_number(&p, &mut errors);
                t.m = get_nr(&mut p, 2);
                if t.m == UNSET {
                    err!(
                        code::ERR_NO_TWO_DIGIT_MONTH,
                        "A two digit month could not be found",
                        begin
                    );
                } else {
                    t.have_date = 1;
                }
            }
            F::TextualMonth3Letter | F::TextualMonthFull => {
                let tmp = lookup_month(&mut p);
                if tmp == 0 {
                    err!(
                        code::ERR_NO_TEXTUAL_MONTH,
                        "A textual month could not be found",
                        begin
                    );
                } else {
                    t.have_date = 1;
                    t.m = tmp;
                }
            }
            F::YearTwoDigit => {
                check_number(&p, &mut errors);
                let (y, length) = get_nr_ex(&mut p, 2);
                t.y = y;
                if y == UNSET {
                    err!(
                        code::ERR_NO_TWO_DIGIT_YEAR,
                        "A two digit year could not be found",
                        begin
                    );
                } else {
                    t.have_date = 1;
                    process_year(&mut t.y, length);
                }
            }
            F::YearFourDigit => {
                check_number(&p, &mut errors);
                t.y = get_nr(&mut p, 4);
                if t.y == UNSET {
                    err!(
                        code::ERR_NO_FOUR_DIGIT_YEAR,
                        "A four digit year could not be found",
                        begin
                    );
                } else {
                    t.have_date = 1;
                }
            }
            F::YearExpanded => {
                check_signed_number(&p, &mut errors);
                t.y = match get_signed_nr(&mut p, 19) {
                    SignedNr::Ok(v) => v,
                    SignedNr::UnexpectedData => {
                        errors.add_error(code::ERR_UNEXPECTED_DATA, 0, 0, "Found unexpected data");
                        0
                    }
                    SignedNr::OutOfRange => {
                        errors.add_error(
                            code::ERR_NUMBER_OUT_OF_RANGE,
                            0,
                            0,
                            "Number out of range",
                        );
                        0
                    }
                };
                if t.y == UNSET {
                    err!(
                        code::ERR_NO_FOUR_DIGIT_YEAR,
                        "An expanded digit year could not be found",
                        begin
                    );
                } else {
                    t.have_date = 1;
                }
            }
            F::HourTwoDigit12Max | F::HourTwoDigit12MaxPadded => {
                check_number(&p, &mut errors);
                t.h = get_nr(&mut p, 2);
                if t.h == UNSET {
                    err!(
                        code::ERR_NO_TWO_DIGIT_HOUR,
                        "A two digit hour could not be found",
                        begin
                    );
                } else if t.h > 12 {
                    err!(
                        code::ERR_HOUR_LARGER_THAN_12,
                        "Hour cannot be higher than 12",
                        begin
                    );
                } else {
                    t.have_time = 1;
                }
            }
            F::HourTwoDigit24MaxPadded | F::HourTwoDigit24Max => {
                check_number(&p, &mut errors);
                t.h = get_nr(&mut p, 2);
                if t.h == UNSET {
                    err!(
                        code::ERR_NO_TWO_DIGIT_HOUR,
                        "A two digit hour could not be found",
                        begin
                    );
                } else {
                    t.have_time = 1;
                }
            }
            F::Meridian => {
                if t.h == UNSET {
                    err!(
                        code::ERR_MERIDIAN_BEFORE_HOUR,
                        "Meridian can only come after an hour has been found",
                        begin
                    );
                }
                let tmp = meridian_with_check(&mut p, t.h);
                if tmp == UNSET {
                    err!(
                        code::ERR_NO_MERIDIAN,
                        "A meridian could not be found",
                        begin
                    );
                } else {
                    t.have_time = 1;
                    if t.h != UNSET {
                        t.h += tmp;
                    }
                }
            }
            F::MinuteTwoDigit => {
                check_number(&p, &mut errors);
                let (min, length) = get_nr_ex(&mut p, 2);
                if min == UNSET || length != 2 {
                    err!(
                        code::ERR_NO_TWO_DIGIT_MINUTE,
                        "A two digit minute could not be found",
                        begin
                    );
                } else {
                    t.have_time = 1;
                    t.i = min;
                }
            }
            F::SecondTwoDigit => {
                check_number(&p, &mut errors);
                let (sec, length) = get_nr_ex(&mut p, 2);
                if sec == UNSET || length != 2 {
                    err!(
                        code::ERR_NO_TWO_DIGIT_SECOND,
                        "A two digit second could not be found",
                        begin
                    );
                } else {
                    t.have_time = 1;
                    t.s = sec;
                }
            }
            F::MicrosecondSixDigit => {
                check_number(&p, &mut errors);
                let tptr = p.p;
                let f_val = get_nr(&mut p, 6);
                if f_val == UNSET || p.p - tptr < 1 {
                    err!(
                        code::ERR_NO_SIX_DIGIT_MICROSECOND,
                        "A six digit microsecond could not be found",
                        begin
                    );
                } else {
                    t.us = (f_val as f64 * 10f64.powf(6.0 - (p.p - tptr) as f64)) as i64;
                }
            }
            F::MillisecondThreeDigit => {
                check_number(&p, &mut errors);
                let tptr = p.p;
                let f_val = get_nr(&mut p, 3);
                if f_val == UNSET || p.p - tptr < 1 {
                    err!(
                        code::ERR_NO_THREE_DIGIT_MILLISECOND,
                        "A three digit millisecond could not be found",
                        begin
                    );
                } else {
                    t.us = (f_val as f64 * 10f64.powf(3.0 - (p.p - tptr) as f64) * 1000.0) as i64;
                }
            }
            F::Whitespace => eat_spaces(&mut p),
            F::EpochSeconds => {
                check_signed_number(&p, &mut errors);
                let tmp = match get_signed_nr(&mut p, 24) {
                    SignedNr::Ok(v) => v,
                    SignedNr::UnexpectedData => {
                        errors.add_error(code::ERR_UNEXPECTED_DATA, 0, 0, "Found unexpected data");
                        0
                    }
                    SignedNr::OutOfRange => {
                        errors.add_error(
                            code::ERR_NUMBER_OUT_OF_RANGE,
                            0,
                            0,
                            "Number out of range",
                        );
                        0
                    }
                };
                t.have_zone = 1;
                t.sse = tmp;
                t.is_localtime = true;
                t.zone_type = ZoneType::Offset;
                t.z = 0;
                t.dst = 0;
                t.update_from_sse();
            }
            F::AnySeparator => {
                if lookup_format(p.c(), map) != F::Separator {
                    err!(
                        code::ERR_NO_SEP_SYMBOL,
                        "The separation symbol ([;:/.,-]) could not be found",
                        begin
                    );
                } else {
                    p.bump();
                }
            }
            F::Separator => {
                if p.c() != f.c() {
                    err!(
                        code::ERR_NO_SEP_SYMBOL,
                        "The separation symbol could not be found",
                        begin
                    );
                } else {
                    p.bump();
                }
            }
            F::ResetAll => reset_fields(&mut t),
            F::ResetAllWhenNotSet => reset_unset_fields(&mut t),
            F::RandomChar => p.bump(),
            F::Escape => {
                if f.at(1) == 0 {
                    err!(
                        code::ERR_EXPECTED_ESCAPE_CHAR,
                        "Escaped character expected",
                        begin
                    );
                } else {
                    f.bump();
                    if p.c() != f.c() {
                        err!(
                            code::ERR_NO_ESCAPED_CHAR,
                            "The escaped character could not be found",
                            begin
                        );
                    } else {
                        p.bump();
                    }
                }
            }
            F::SkipToSeparator => eat_until_separator(&mut p),
            F::AllowExtraCharacters => allow_extra = true,
            F::YearIso => {
                iso_year = get_nr(&mut p, 4);
                if iso_year == UNSET {
                    err!(
                        code::ERR_NO_FOUR_DIGIT_YEAR_ISO,
                        "A four digit ISO year could not be found",
                        begin
                    );
                } else {
                    t.have_date = 1;
                }
            }
            F::WeekOfYearIso => {
                iso_week_of_year = get_nr(&mut p, 2);
                if iso_week_of_year == UNSET {
                    err!(
                        code::ERR_NO_TWO_DIGIT_WEEK,
                        "A two digit ISO week could not be found",
                        begin
                    );
                } else if !(1..=53).contains(&iso_week_of_year) {
                    err!(
                        code::ERR_INVALID_WEEK,
                        "ISO Week must be between 1 and 53",
                        begin
                    );
                } else {
                    t.have_date = 1;
                }
            }
            F::DayOfWeekIso => {
                iso_day_of_week = get_nr(&mut p, 1);
                if iso_day_of_week == UNSET {
                    err!(
                        code::ERR_NO_DAY_OF_WEEK,
                        "A single digit day of week could not be found",
                        begin
                    );
                } else if !(1..=7).contains(&iso_day_of_week) {
                    err!(
                        code::ERR_INVALID_DAY_OF_WEEK,
                        "Day of week must be between 1 and 7",
                        begin
                    );
                } else {
                    t.have_date = 1;
                }
            }
            F::TimezoneOffset => {
                let (z, not_found) = parse_zone(&mut p, &mut t, tz_get);
                t.z = z as i32;
                if not_found {
                    err!(
                        code::ERR_TZID_NOT_FOUND,
                        "The timezone could not be found in the database",
                        begin
                    );
                } else {
                    t.have_zone = 1;
                }
            }
            F::TimezoneOffsetMinutes => {
                let z = parse_tz_minutes(&mut p, &mut t);
                t.z = z as i32;
                if z == UNSET {
                    err!(
                        code::ERR_INVALID_TZ_OFFSET,
                        "Invalid timezone offset in minutes",
                        begin
                    );
                } else {
                    t.have_zone = 1;
                }
            }
            F::Literal | F::End | F::DayOfWeek | F::WeekOfYear => {
                if f.c() != p.c() {
                    err!(
                        code::ERR_WRONG_FORMAT_SEP,
                        "The format separator does not match",
                        begin
                    );
                }
                p.bump();
            }
        }
        f.bump();
    }

    if p.c() != 0 {
        if allow_extra {
            warn!(code::WARN_TRAILING_DATA, "Trailing data", p.p);
        } else {
            err!(code::ERR_TRAILING_DATA, "Trailing data", p.p);
        }
    }

    if f.c() != 0 {
        // Trailing reset specifiers are valid.
        while f.c() != 0 {
            match lookup_format(f.c(), map) {
                F::ResetAll => reset_fields(&mut t),
                F::ResetAllWhenNotSet => reset_unset_fields(&mut t),
                F::AllowExtraCharacters => {}
                _ => {
                    err!(
                        code::ERR_DATA_MISSING,
                        "Not enough data available to satisfy format",
                        p.p
                    );
                    break;
                }
            }
            f.bump();
        }
    }

    // clean up a bit
    if t.h != UNSET || t.i != UNSET || t.s != UNSET || t.us != UNSET {
        if t.h == UNSET {
            t.h = 0;
        }
        if t.i == UNSET {
            t.i = 0;
        }
        if t.s == UNSET {
            t.s = 0;
        }
        if t.us == UNSET {
            t.us = 0;
        }
    }

    // Check for mixing of ISO dates with natural dates.
    if t.y != UNSET && (iso_week_of_year != UNSET || iso_year != UNSET || iso_day_of_week != UNSET)
    {
        err!(
            code::ERR_MIX_ISO_WITH_NATURAL,
            "Mixing of ISO dates with natural dates is not allowed",
            p.p
        );
    }
    if iso_year != UNSET && (t.y != UNSET || t.m != UNSET || t.d != UNSET) {
        err!(
            code::ERR_MIX_ISO_WITH_NATURAL,
            "Mixing of ISO dates with natural dates is not allowed",
            p.p
        );
    }

    // Convert ISO values
    if iso_year != UNSET {
        if iso_week_of_year == UNSET {
            iso_week_of_year = 1;
        }
        if iso_day_of_week == UNSET {
            iso_day_of_week = 1;
        }
        let (y, m, d) = date_from_isodate(iso_year, iso_week_of_year, iso_day_of_week);
        t.y = y;
        t.m = m;
        t.d = d;
    } else if iso_week_of_year != UNSET || iso_day_of_week != UNSET {
        warn!(code::WARN_INVALID_DATE, "The parsed date was invalid", p.p);
    }

    // do funky checking whether the parsed time was valid time
    if t.h != UNSET && t.i != UNSET && t.s != UNSET && !valid_time(t.h, t.i, t.s) {
        warn!(code::WARN_INVALID_TIME, "The parsed time was invalid", p.p);
    }
    // do funky checking whether the parsed date was valid date
    if t.y != UNSET && t.m != UNSET && t.d != UNSET && !valid_date(t.y, t.m, t.d) {
        warn!(code::WARN_INVALID_DATE, "The parsed date was invalid", p.p);
    }

    ParseResult { time: t, errors }
}
