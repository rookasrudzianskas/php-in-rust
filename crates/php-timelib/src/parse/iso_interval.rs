//! ISO 8601 interval parsing for `DatePeriod`/`DateInterval` (`parse_iso_intervals.re`).

use std::sync::OnceLock;

use super::common::{Cur, get_nr, is_c_space};
use super::regex::{Re, Scanner, alt, any, class, ilit, lit, opt, plus, rep, seq};
use crate::error::ErrorContainer;
use crate::time::{RelTime, Time, UNSET, ZoneType};

const YYMAXFILL: usize = 20;

/// Result of [`strtointerval`].
#[derive(Clone, Debug, Default)]
pub struct IsoInterval {
    /// Start date (`2008-03-01T13:00:00Z`), if present.
    pub begin: Option<Time>,
    /// End date, if present.
    pub end: Option<Time>,
    /// The period (`P1Y2M10DT2H30M`), if present.
    pub period: Option<RelTime>,
    /// Number of recurrences (`R5`), if present.
    pub recurrences: Option<i32>,
    /// Errors (the interval parser never produces warnings).
    pub errors: ErrorContainer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Recurrences,
    DateTime,
    Period,
    CombinedRep,
    Separator,
    NulNewline,
    Any,
}

const RULES: [Rule; 7] = [
    Rule::Recurrences,
    Rule::DateTime,
    Rule::Period,
    Rule::CombinedRep,
    Rule::Separator,
    Rule::NulNewline,
    Rule::Any,
];

fn byte(b: u8) -> Re {
    let mut set = super::regex::ByteSet::empty();
    set.insert(b);
    Re::Set(set)
}

fn scanner() -> &'static Scanner {
    static SCANNER: OnceLock<Scanner> = OnceLock::new();
    SCANNER.get_or_init(|| {
        let digit = || class("0-9");
        let number = || plus(digit());
        let hour24lz = || {
            alt(vec![
                seq(vec![class("01"), digit()]),
                seq(vec![lit("2"), class("0-4")]),
            ])
        };
        let minutelz = || seq(vec![class("0-5"), digit()]);
        let monthlz = || {
            alt(vec![
                seq(vec![lit("0"), class("1-9")]),
                seq(vec![lit("1"), class("0-2")]),
            ])
        };
        let monthlzz = || {
            alt(vec![
                seq(vec![lit("0"), digit()]),
                seq(vec![lit("1"), class("0-2")]),
            ])
        };
        let daylz = || {
            alt(vec![
                seq(vec![lit("0"), class("1-9")]),
                seq(vec![class("1-2"), digit()]),
                seq(vec![lit("3"), class("01")]),
            ])
        };
        let daylzz = || {
            alt(vec![
                seq(vec![lit("0"), digit()]),
                seq(vec![class("1-2"), digit()]),
                seq(vec![lit("3"), class("01")]),
            ])
        };
        let secondlz = minutelz;
        let year4 = || rep(digit(), 4, 4);
        let datetimebasic = seq(vec![
            year4(),
            monthlz(),
            daylz(),
            lit("T"),
            hour24lz(),
            minutelz(),
            secondlz(),
            lit("Z"),
        ]);
        let datetimeextended = seq(vec![
            year4(),
            lit("-"),
            monthlz(),
            lit("-"),
            daylz(),
            lit("T"),
            hour24lz(),
            ilit(":"),
            minutelz(),
            ilit(":"),
            secondlz(),
            lit("Z"),
        ]);
        let unit = |c: &str| opt(seq(vec![number(), lit(c)]));
        let period = seq(vec![
            lit("P"),
            unit("Y"),
            unit("M"),
            unit("W"),
            unit("D"),
            opt(seq(vec![lit("T"), unit("H"), unit("M"), unit("S")])),
        ]);
        let combinedrep = seq(vec![
            lit("P"),
            year4(),
            lit("-"),
            monthlzz(),
            lit("-"),
            daylzz(),
            lit("T"),
            hour24lz(),
            ilit(":"),
            minutelz(),
            ilit(":"),
            secondlz(),
        ]);
        let recurrences = seq(vec![lit("R"), number()]);
        let rules = vec![
            recurrences,
            alt(vec![datetimebasic, datetimeextended]),
            period,
            combinedrep,
            class(" .,\t/"),
            alt(vec![byte(0), lit("\n")]),
            any(),
        ];
        Scanner::new(&rules)
    })
}

/// `timelib_get_unsigned_nr` (which, despite its name, accepts signs).
fn get_unsigned_nr(c: &mut Cur, max_length: usize) -> i64 {
    let mut dir: i64 = 1;
    while !c.c().is_ascii_digit() && c.c() != b'+' && c.c() != b'-' {
        if c.c() == 0 {
            return UNSET;
        }
        c.bump();
    }
    while c.c() == b'+' || c.c() == b'-' {
        if c.c() == b'-' {
            dir = -dir;
        }
        c.bump();
    }
    dir.wrapping_mul(get_nr(c, max_length))
}

fn new_endpoint() -> Time {
    let mut t = Time::new_unset();
    t.us = 0;
    t.z = 0;
    t.dst = 0;
    t.is_localtime = false;
    t.zone_type = ZoneType::Offset;
    t
}

/// Parses an ISO 8601 interval specification such as
/// `R5/2008-03-01T13:00:00Z/P1Y2M10DT2H30M` (`timelib_strtointerval`).
pub fn strtointerval(input: impl AsRef<[u8]>) -> IsoInterval {
    let input = input.as_ref();
    let mut out = IsoInterval::default();
    if input.is_empty() {
        out.errors.add_error(0, 0, 0, "Empty string");
        return out;
    }
    let mut s = 0usize;
    let mut e = input.len() - 1;
    while is_c_space(input[s]) && s < e {
        s += 1;
    }
    while is_c_space(input[e]) && e > s {
        e -= 1;
    }
    let len = e + 1 - s;
    let mut buf = Vec::with_capacity(len + YYMAXFILL);
    buf.extend_from_slice(&input[s..=e]);
    buf.resize(len + YYMAXFILL, 0);

    let mut begin = new_endpoint();
    let mut end = new_endpoint();
    let mut period = RelTime {
        days: UNSET,
        ..RelTime::default()
    };
    let mut recurrences = 1i32;
    let (mut have_period, mut have_recurrences, mut have_date) = (false, false, false);
    let (mut have_begin_date, mut have_end_date) = (false, false);
    let errors = &mut out.errors;

    let mut cursor = 0usize;
    loop {
        let tok = cursor;
        if buf.len() - cursor < YYMAXFILL {
            break;
        }
        let (tok_end, rule) = match scanner().longest_match(&buf, cursor) {
            Some((end, idx)) => (end, RULES[idx]),
            None => (cursor + 1, Rule::Any),
        };
        cursor = tok_end;
        let token = buf[tok..tok_end].to_vec();
        let tok_char = buf[tok];
        let mut c = Cur::new(&token);
        match rule {
            Rule::Recurrences => {
                c.bump();
                recurrences = get_unsigned_nr(&mut c, 9) as i32;
                have_recurrences = true;
            }
            Rule::DateTime => {
                let current = if have_date || have_period {
                    have_end_date = true;
                    &mut end
                } else {
                    have_begin_date = true;
                    &mut begin
                };
                current.y = get_nr(&mut c, 4);
                current.m = get_nr(&mut c, 2);
                current.d = get_nr(&mut c, 2);
                current.h = get_nr(&mut c, 2);
                current.i = get_nr(&mut c, 2);
                current.s = get_nr(&mut c, 2);
                have_date = true;
            }
            Rule::Period => {
                let mut in_time = false;
                c.bump();
                loop {
                    if c.c() == b'T' {
                        in_time = true;
                        c.bump();
                    }
                    if c.c() == 0 {
                        errors.add_error(0, tok, tok_char, "Missing expected time part");
                        break;
                    }
                    let nr = get_unsigned_nr(&mut c, 12);
                    match c.c() {
                        b'Y' => period.y = nr,
                        b'W' => period.d = period.d.wrapping_add(nr.wrapping_mul(7)),
                        b'D' => period.d = period.d.wrapping_add(nr),
                        b'H' => period.h = nr,
                        b'S' => period.s = nr,
                        b'M' => {
                            if in_time {
                                period.i = nr;
                            } else {
                                period.m = nr;
                            }
                        }
                        _ => errors.add_error(0, tok, tok_char, "Undefined period specifier"),
                    }
                    c.bump();
                    if !errors.errors.is_empty() || c.c() == 0 {
                        break;
                    }
                }
                have_period = true;
            }
            Rule::CombinedRep => {
                period.y = get_unsigned_nr(&mut c, 4);
                c.bump();
                period.m = get_unsigned_nr(&mut c, 2);
                c.bump();
                period.d = get_unsigned_nr(&mut c, 2);
                c.bump();
                period.h = get_unsigned_nr(&mut c, 2);
                c.bump();
                period.i = get_unsigned_nr(&mut c, 2);
                c.bump();
                period.s = get_unsigned_nr(&mut c, 2);
                have_period = true;
            }
            Rule::Separator | Rule::NulNewline => {}
            Rule::Any => errors.add_error(0, tok, tok_char, "Unexpected character"),
        }
    }

    if have_begin_date {
        out.begin = Some(begin);
    }
    if have_end_date {
        out.end = Some(end);
    }
    if have_period {
        out.period = Some(period);
    }
    if have_recurrences {
        out.recurrences = Some(recurrences);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_spec() {
        let r = strtointerval("R5/2008-03-01T13:00:00Z/P1Y2M10DT2H30M");
        assert!(r.errors.errors.is_empty());
        assert_eq!(r.recurrences, Some(5));
        let b = r.begin.unwrap();
        assert_eq!((b.y, b.m, b.d, b.h, b.i, b.s), (2008, 3, 1, 13, 0, 0));
        let p = r.period.unwrap();
        assert_eq!((p.y, p.m, p.d, p.h, p.i, p.s), (1, 2, 10, 2, 30, 0));
        assert!(r.end.is_none());
    }

    #[test]
    fn periods() {
        let p = strtointerval("P2W").period.unwrap();
        assert_eq!(p.d, 14);
        let p = strtointerval("PT36H").period.unwrap();
        assert_eq!(p.h, 36);
        let p = strtointerval("P0001-02-03T04:05:06").period.unwrap();
        assert_eq!((p.y, p.m, p.d, p.h, p.i, p.s), (1, 2, 3, 4, 5, 6));
        let r = strtointerval("P");
        assert_eq!(r.errors.errors[0].message, "Missing expected time part");
        let r = strtointerval("xP1D");
        assert_eq!(r.errors.errors[0].message, "Unexpected character");
        assert_eq!(strtointerval("").errors.errors[0].message, "Empty string");
    }

    #[test]
    fn start_end() {
        let r = strtointerval("2008-03-01T13:00:00Z/2008-05-11T15:30:00Z");
        assert_eq!(r.begin.unwrap().m, 3);
        assert_eq!(r.end.unwrap().m, 5);
    }
}
