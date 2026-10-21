//! The strtotime grammar from `parse_date.re`, expressed with [`super::regex`]
//! combinators. Every definition mirrors its re2c counterpart; rules are listed in the
//! same order as in the re2c source because ties are resolved by rule order.

use super::regex::{Re, Scanner, alt, any, class, ilit, lit, opt, plus, rep, seq, star};
use std::sync::OnceLock;

/// The scanner rules, in re2c priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    Yesterday,
    Now,
    Noon,
    MidnightToday,
    Tomorrow,
    Timestamp,
    TimestampMs,
    FirstLastDayOf,
    BackFrontOf,
    WeekdayOf,
    Time12,
    MssqlTime,
    Time24,
    GnuNoColon,
    Iso8601NoColon,
    American,
    IsoDate4,
    IsoDate2,
    IsoDateX,
    GnuDateShorter,
    GnuDateShort,
    DateFull,
    PointedDate4,
    PointedDate2,
    DateNoDay,
    DateNoDayRev,
    DateTextual,
    DateNoYearRev,
    DateNoColon,
    XmlRpc,
    PgYdotd,
    IsoWeekDay,
    IsoWeek,
    PgTextShort,
    PgTextReverse,
    Clf,
    Year4,
    Ago,
    DayText,
    RelativeTextWeek,
    RelativeText,
    MonthText,
    Tz,
    DateShortWithTime12,
    DateShortWithTime24,
    Relative,
    Separator,
    Space,
    NulNewline,
    Any,
}

const RULES: [Rule; 50] = [
    Rule::Yesterday,
    Rule::Now,
    Rule::Noon,
    Rule::MidnightToday,
    Rule::Tomorrow,
    Rule::Timestamp,
    Rule::TimestampMs,
    Rule::FirstLastDayOf,
    Rule::BackFrontOf,
    Rule::WeekdayOf,
    Rule::Time12,
    Rule::MssqlTime,
    Rule::Time24,
    Rule::GnuNoColon,
    Rule::Iso8601NoColon,
    Rule::American,
    Rule::IsoDate4,
    Rule::IsoDate2,
    Rule::IsoDateX,
    Rule::GnuDateShorter,
    Rule::GnuDateShort,
    Rule::DateFull,
    Rule::PointedDate4,
    Rule::PointedDate2,
    Rule::DateNoDay,
    Rule::DateNoDayRev,
    Rule::DateTextual,
    Rule::DateNoYearRev,
    Rule::DateNoColon,
    Rule::XmlRpc,
    Rule::PgYdotd,
    Rule::IsoWeekDay,
    Rule::IsoWeek,
    Rule::PgTextShort,
    Rule::PgTextReverse,
    Rule::Clf,
    Rule::Year4,
    Rule::Ago,
    Rule::DayText,
    Rule::RelativeTextWeek,
    Rule::RelativeText,
    Rule::MonthText,
    Rule::Tz,
    Rule::DateShortWithTime12,
    Rule::DateShortWithTime24,
    Rule::Relative,
    Rule::Separator,
    Rule::Space,
    Rule::NulNewline,
    Rule::Any,
];

fn ilits(words: &[&str]) -> Re {
    alt(words.iter().map(|w| ilit(w)).collect())
}

fn lits(words: &[&str]) -> Re {
    alt(words.iter().map(|w| lit(w)).collect())
}

fn digit() -> Re {
    class("0-9")
}

fn digits(min: u32, max: u32) -> Re {
    rep(digit(), min, max)
}

/// All named sub-expressions of the grammar.
struct Defs;

impl Defs {
    fn space() -> Re {
        alt(vec![
            plus(class(" \t")),
            plus(seq(vec![byte(0xc2), byte(0xa0)])),
            plus(seq(vec![byte(0xe2), byte(0x80), byte(0xaf)])),
        ])
    }
    fn frac() -> Re {
        seq(vec![lit("."), plus(digit())])
    }
    fn hour24() -> Re {
        alt(vec![
            seq(vec![opt(class("01")), digit()]),
            seq(vec![lit("2"), class("0-4")]),
        ])
    }
    fn hour24lz() -> Re {
        alt(vec![
            seq(vec![class("01"), digit()]),
            seq(vec![lit("2"), class("0-4")]),
        ])
    }
    fn hour12() -> Re {
        alt(vec![
            seq(vec![opt(lit("0")), class("1-9")]),
            seq(vec![lit("1"), class("0-2")]),
        ])
    }
    fn minute() -> Re {
        seq(vec![opt(class("0-5")), digit()])
    }
    fn minutelz() -> Re {
        seq(vec![class("0-5"), digit()])
    }
    fn second() -> Re {
        alt(vec![Self::minute(), lit("60")])
    }
    fn secondlz() -> Re {
        alt(vec![Self::minutelz(), lit("60")])
    }
    fn meridian() -> Re {
        seq(vec![
            class("AaPp"),
            opt(lit(".")),
            class("Mm"),
            opt(lit(".")),
            alt(vec![byte(0), class("\t ")]),
        ])
    }
    fn tz() -> Re {
        alt(vec![
            seq(vec![
                opt(lit("(")),
                rep(class("A-Za-z"), 1, 6),
                opt(lit(")")),
            ]),
            seq(vec![
                class("A-Z"),
                plus(class("a-z")),
                plus(seq(vec![class("_/-"), plus(class("A-Za-z"))])),
            ]),
        ])
    }
    fn tzcorrection() -> Re {
        seq(vec![
            opt(lit("GMT")),
            class("+-"),
            alt(vec![
                seq(vec![
                    Self::hour24(),
                    opt(seq(vec![opt(lit(":")), Self::minute()])),
                ]),
                seq(vec![Self::hour24lz(), Self::minutelz(), Self::secondlz()]),
                seq(vec![
                    Self::hour24lz(),
                    lit(":"),
                    Self::minutelz(),
                    lit(":"),
                    Self::secondlz(),
                ]),
            ]),
        ])
    }
    fn daysuf() -> Re {
        lits(&["st", "nd", "rd", "th"])
    }
    fn month() -> Re {
        alt(vec![
            seq(vec![opt(lit("0")), digit()]),
            seq(vec![lit("1"), class("0-2")]),
        ])
    }
    fn day() -> Re {
        seq(vec![
            alt(vec![
                seq(vec![opt(class("0-2")), digit()]),
                seq(vec![lit("3"), class("01")]),
            ]),
            opt(Self::daysuf()),
        ])
    }
    fn year() -> Re {
        digits(1, 4)
    }
    fn year2() -> Re {
        digits(2, 2)
    }
    fn year4() -> Re {
        digits(4, 4)
    }
    fn year4withsign() -> Re {
        seq(vec![opt(class("+-")), digits(4, 4)])
    }
    fn yearx() -> Re {
        seq(vec![class("+-"), digits(5, 19)])
    }
    fn dayofyear() -> Re {
        alt(vec![
            seq(vec![lit("00"), class("1-9")]),
            seq(vec![lit("0"), class("1-9"), digit()]),
            seq(vec![class("1-2"), digit(), digit()]),
            seq(vec![lit("3"), class("0-5"), digit()]),
            seq(vec![lit("36"), class("0-6")]),
        ])
    }
    fn weekofyear() -> Re {
        alt(vec![
            seq(vec![lit("0"), class("1-9")]),
            seq(vec![class("1-4"), digit()]),
            seq(vec![lit("5"), class("0-3")]),
        ])
    }
    fn monthlz() -> Re {
        alt(vec![
            seq(vec![lit("0"), digit()]),
            seq(vec![lit("1"), class("0-2")]),
        ])
    }
    fn daylz() -> Re {
        alt(vec![
            seq(vec![lit("0"), digit()]),
            seq(vec![class("1-2"), digit()]),
            seq(vec![lit("3"), class("01")]),
        ])
    }
    fn dayfulls() -> Re {
        ilits(&[
            "sundays",
            "mondays",
            "tuesdays",
            "wednesdays",
            "thursdays",
            "fridays",
            "saturdays",
        ])
    }
    fn dayfull() -> Re {
        ilits(&[
            "sunday",
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
        ])
    }
    fn dayabbr() -> Re {
        ilits(&["sun", "mon", "tue", "wed", "thu", "fri", "sat", "sun"])
    }
    fn dayspecial() -> Re {
        ilits(&["weekday", "weekdays"])
    }
    fn daytext() -> Re {
        alt(vec![
            Self::dayfulls(),
            Self::dayfull(),
            Self::dayabbr(),
            Self::dayspecial(),
        ])
    }
    fn monthfull() -> Re {
        ilits(&[
            "january",
            "february",
            "march",
            "april",
            "may",
            "june",
            "july",
            "august",
            "september",
            "october",
            "november",
            "december",
        ])
    }
    fn monthabbr() -> Re {
        ilits(&[
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "sept", "oct", "nov",
            "dec",
        ])
    }
    fn monthroman() -> Re {
        lits(&[
            "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII",
        ])
    }
    fn monthtext() -> Re {
        alt(vec![
            Self::monthfull(),
            Self::monthabbr(),
            Self::monthroman(),
        ])
    }

    // Time formats
    fn timetiny12() -> Re {
        seq(vec![Self::hour12(), opt(Self::space()), Self::meridian()])
    }
    fn timeshort12() -> Re {
        seq(vec![
            Self::hour12(),
            class(":."),
            Self::minutelz(),
            opt(Self::space()),
            Self::meridian(),
        ])
    }
    fn timelong12() -> Re {
        seq(vec![
            Self::hour12(),
            class(":."),
            Self::minute(),
            class(":."),
            Self::secondlz(),
            opt(Self::space()),
            Self::meridian(),
        ])
    }
    fn timetiny24() -> Re {
        seq(vec![ilit("t"), Self::hour24()])
    }
    fn timeshort24() -> Re {
        seq(vec![
            opt(ilit("t")),
            Self::hour24(),
            class(":."),
            Self::minute(),
        ])
    }
    fn timelong24() -> Re {
        seq(vec![
            opt(ilit("t")),
            Self::hour24(),
            class(":."),
            Self::minute(),
            class(":."),
            Self::second(),
        ])
    }
    fn iso8601long() -> Re {
        seq(vec![
            opt(ilit("t")),
            Self::hour24(),
            class(":."),
            Self::minute(),
            class(":."),
            Self::second(),
            Self::frac(),
        ])
    }
    fn iso8601normtz() -> Re {
        seq(vec![
            opt(ilit("t")),
            Self::hour24(),
            class(":."),
            Self::minute(),
            class(":."),
            Self::secondlz(),
            opt(Self::space()),
            alt(vec![Self::tzcorrection(), Self::tz()]),
        ])
    }
    fn gnunocolon() -> Re {
        seq(vec![opt(ilit("t")), Self::hour24lz(), Self::minutelz()])
    }
    fn iso8601nocolon() -> Re {
        seq(vec![
            opt(ilit("t")),
            Self::hour24lz(),
            Self::minutelz(),
            Self::secondlz(),
        ])
    }

    // Date formats
    fn americanshort() -> Re {
        seq(vec![Self::month(), lit("/"), Self::day()])
    }
    fn american() -> Re {
        seq(vec![
            Self::month(),
            lit("/"),
            Self::day(),
            lit("/"),
            Self::year(),
        ])
    }
    fn iso8601dateslash() -> Re {
        seq(vec![
            Self::year4(),
            lit("/"),
            Self::monthlz(),
            lit("/"),
            Self::daylz(),
            opt(lit("/")),
        ])
    }
    fn dateslash() -> Re {
        seq(vec![
            Self::year4(),
            lit("/"),
            Self::month(),
            lit("/"),
            Self::day(),
        ])
    }
    fn iso8601date4() -> Re {
        seq(vec![
            Self::year4withsign(),
            lit("-"),
            Self::monthlz(),
            lit("-"),
            Self::daylz(),
        ])
    }
    fn iso8601date2() -> Re {
        seq(vec![
            Self::year2(),
            lit("-"),
            Self::monthlz(),
            lit("-"),
            Self::daylz(),
        ])
    }
    fn iso8601datex() -> Re {
        seq(vec![
            Self::yearx(),
            lit("-"),
            Self::monthlz(),
            lit("-"),
            Self::daylz(),
        ])
    }
    fn gnudateshorter() -> Re {
        seq(vec![Self::year4(), lit("-"), Self::month()])
    }
    fn gnudateshort() -> Re {
        seq(vec![
            Self::year(),
            lit("-"),
            Self::month(),
            lit("-"),
            Self::day(),
        ])
    }
    fn pointeddate4() -> Re {
        seq(vec![
            Self::day(),
            class(".\t-"),
            Self::month(),
            class(".-"),
            Self::year4(),
        ])
    }
    fn pointeddate2() -> Re {
        seq(vec![
            Self::day(),
            class(".\t"),
            Self::month(),
            lit("."),
            Self::year2(),
        ])
    }
    fn datefull() -> Re {
        seq(vec![
            Self::day(),
            star(class(" \t.-")),
            Self::monthtext(),
            star(class(" \t.-")),
            Self::year(),
        ])
    }
    fn datenoday() -> Re {
        seq(vec![Self::monthtext(), star(class(" .\t-")), Self::year4()])
    }
    fn datenodayrev() -> Re {
        seq(vec![Self::year4(), star(class(" .\t-")), Self::monthtext()])
    }
    fn datetextual() -> Re {
        seq(vec![
            Self::monthtext(),
            star(class(" .\t-")),
            Self::day(),
            plus(class(",.stndrh\t ")),
            Self::year(),
        ])
    }
    fn datenoyear() -> Re {
        seq(vec![
            Self::monthtext(),
            star(class(" .\t-")),
            Self::day(),
            alt(vec![plus(class(",.stndrh\t ")), byte(0)]),
        ])
    }
    fn datenoyearrev() -> Re {
        seq(vec![Self::day(), star(class(" .\t-")), Self::monthtext()])
    }
    fn datenocolon() -> Re {
        seq(vec![Self::year4(), Self::monthlz(), Self::daylz()])
    }

    // Special formats
    fn soap() -> Re {
        seq(vec![
            Self::year4(),
            lit("-"),
            Self::monthlz(),
            lit("-"),
            Self::daylz(),
            lit("T"),
            Self::hour24lz(),
            lit(":"),
            Self::minutelz(),
            lit(":"),
            Self::secondlz(),
            Self::frac(),
            opt(Self::tzcorrection()),
        ])
    }
    fn xmlrpc() -> Re {
        seq(vec![
            Self::year4(),
            Self::monthlz(),
            Self::daylz(),
            lit("T"),
            Self::hour24(),
            lit(":"),
            Self::minutelz(),
            lit(":"),
            Self::secondlz(),
        ])
    }
    fn xmlrpcnocolon() -> Re {
        seq(vec![
            Self::year4(),
            Self::monthlz(),
            Self::daylz(),
            ilit("t"),
            Self::hour24(),
            Self::minutelz(),
            Self::secondlz(),
        ])
    }
    fn wddx() -> Re {
        seq(vec![
            Self::year4(),
            lit("-"),
            Self::month(),
            lit("-"),
            Self::day(),
            lit("T"),
            Self::hour24(),
            lit(":"),
            Self::minute(),
            lit(":"),
            Self::second(),
        ])
    }
    fn pgydotd() -> Re {
        seq(vec![Self::year4(), opt(class(".-")), Self::dayofyear()])
    }
    fn pgtextshort() -> Re {
        seq(vec![
            Self::monthabbr(),
            lit("-"),
            Self::daylz(),
            lit("-"),
            Self::year(),
        ])
    }
    fn pgtextreverse() -> Re {
        seq(vec![
            Self::year(),
            lit("-"),
            Self::monthabbr(),
            lit("-"),
            Self::daylz(),
        ])
    }
    fn mssqltime() -> Re {
        seq(vec![
            Self::hour12(),
            lit(":"),
            Self::minutelz(),
            lit(":"),
            Self::secondlz(),
            class(":."),
            plus(digit()),
            Self::meridian(),
        ])
    }
    fn isoweekday() -> Re {
        seq(vec![
            Self::year4(),
            opt(lit("-")),
            lit("W"),
            Self::weekofyear(),
            opt(lit("-")),
            class("0-7"),
        ])
    }
    fn isoweek() -> Re {
        seq(vec![
            Self::year4(),
            opt(lit("-")),
            lit("W"),
            Self::weekofyear(),
        ])
    }
    fn exif() -> Re {
        seq(vec![
            Self::year4(),
            lit(":"),
            Self::monthlz(),
            lit(":"),
            Self::daylz(),
            lit(" "),
            Self::hour24lz(),
            lit(":"),
            Self::minutelz(),
            lit(":"),
            Self::secondlz(),
        ])
    }
    fn firstdayof() -> Re {
        ilit("first day of")
    }
    fn lastdayof() -> Re {
        ilit("last day of")
    }
    fn backof() -> Re {
        seq(vec![
            ilit("back of "),
            Self::hour24(),
            opt(seq(vec![opt(Self::space()), Self::meridian()])),
        ])
    }
    fn frontof() -> Re {
        seq(vec![
            ilit("front of "),
            Self::hour24(),
            opt(seq(vec![opt(Self::space()), Self::meridian()])),
        ])
    }
    fn clf() -> Re {
        seq(vec![
            Self::day(),
            lit("/"),
            Self::monthabbr(),
            lit("/"),
            Self::year4(),
            lit(":"),
            Self::hour24lz(),
            lit(":"),
            Self::minutelz(),
            lit(":"),
            Self::secondlz(),
            Self::space(),
            Self::tzcorrection(),
        ])
    }
    fn timestamp() -> Re {
        seq(vec![lit("@"), opt(lit("-")), plus(digit())])
    }
    fn timestampms() -> Re {
        seq(vec![
            lit("@"),
            opt(lit("-")),
            plus(digit()),
            lit("."),
            digits(0, 6),
        ])
    }
    fn dateshortwithtimeshort12() -> Re {
        seq(vec![Self::datenoyear(), Self::timeshort12()])
    }
    fn dateshortwithtimelong12() -> Re {
        seq(vec![Self::datenoyear(), Self::timelong12()])
    }
    fn dateshortwithtimeshort() -> Re {
        seq(vec![Self::datenoyear(), Self::timeshort24()])
    }
    fn dateshortwithtimelong() -> Re {
        seq(vec![Self::datenoyear(), Self::timelong24()])
    }
    fn dateshortwithtimelongtz() -> Re {
        seq(vec![Self::datenoyear(), Self::iso8601normtz()])
    }

    // Relative regexps
    fn reltextnumber() -> Re {
        ilits(&[
            "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eight", "eighth",
            "ninth", "tenth", "eleventh", "twelfth",
        ])
    }
    fn reltexttext() -> Re {
        ilits(&["next", "last", "previous", "this"])
    }
    fn reltextunit() -> Re {
        alt(vec![
            ilit("ms"),
            ilit("\u{b5}s"),
            seq(vec![
                ilits(&[
                    "msec",
                    "millisecond",
                    "\u{b5}sec",
                    "microsecond",
                    "usec",
                    "sec",
                    "second",
                    "min",
                    "minute",
                    "hour",
                    "day",
                    "fortnight",
                    "forthnight",
                    "month",
                    "year",
                ]),
                opt(ilit("s")),
            ]),
            ilit("weeks"),
            Self::daytext(),
        ])
    }
    fn relnumber() -> Re {
        seq(vec![star(class("+-")), star(class(" \t")), digits(1, 13)])
    }
    fn relative() -> Re {
        seq(vec![
            Self::relnumber(),
            opt(Self::space()),
            alt(vec![Self::reltextunit(), ilit("week")]),
        ])
    }
    fn relativetext() -> Re {
        seq(vec![
            alt(vec![Self::reltextnumber(), Self::reltexttext()]),
            Self::space(),
            Self::reltextunit(),
        ])
    }
    fn relativetextweek() -> Re {
        seq(vec![Self::reltexttext(), Self::space(), ilit("week")])
    }
    fn weekdayof() -> Re {
        seq(vec![
            alt(vec![Self::reltextnumber(), Self::reltexttext()]),
            Self::space(),
            alt(vec![Self::dayfulls(), Self::dayfull(), Self::dayabbr()]),
            Self::space(),
            ilit("of"),
        ])
    }
}

fn byte(b: u8) -> Re {
    let mut set = super::regex::ByteSet::empty();
    set.insert(b);
    Re::Set(set)
}

fn rule_re(rule: Rule) -> Re {
    type D = Defs;
    match rule {
        Rule::Yesterday => ilit("yesterday"),
        Rule::Now => ilit("now"),
        Rule::Noon => ilit("noon"),
        Rule::MidnightToday => ilits(&["midnight", "today"]),
        Rule::Tomorrow => ilit("tomorrow"),
        Rule::Timestamp => D::timestamp(),
        Rule::TimestampMs => D::timestampms(),
        Rule::FirstLastDayOf => alt(vec![D::firstdayof(), D::lastdayof()]),
        Rule::BackFrontOf => alt(vec![D::backof(), D::frontof()]),
        Rule::WeekdayOf => D::weekdayof(),
        Rule::Time12 => alt(vec![D::timetiny12(), D::timeshort12(), D::timelong12()]),
        Rule::MssqlTime => D::mssqltime(),
        Rule::Time24 => alt(vec![
            D::timetiny24(),
            D::timeshort24(),
            D::timelong24(),
            D::iso8601long(),
        ]),
        Rule::GnuNoColon => D::gnunocolon(),
        Rule::Iso8601NoColon => D::iso8601nocolon(),
        Rule::American => alt(vec![D::americanshort(), D::american()]),
        Rule::IsoDate4 => alt(vec![
            D::iso8601date4(),
            D::iso8601dateslash(),
            D::dateslash(),
        ]),
        Rule::IsoDate2 => D::iso8601date2(),
        Rule::IsoDateX => D::iso8601datex(),
        Rule::GnuDateShorter => D::gnudateshorter(),
        Rule::GnuDateShort => D::gnudateshort(),
        Rule::DateFull => D::datefull(),
        Rule::PointedDate4 => D::pointeddate4(),
        Rule::PointedDate2 => D::pointeddate2(),
        Rule::DateNoDay => D::datenoday(),
        Rule::DateNoDayRev => D::datenodayrev(),
        Rule::DateTextual => alt(vec![D::datetextual(), D::datenoyear()]),
        Rule::DateNoYearRev => D::datenoyearrev(),
        Rule::DateNoColon => D::datenocolon(),
        Rule::XmlRpc => alt(vec![
            D::xmlrpc(),
            D::xmlrpcnocolon(),
            D::soap(),
            D::wddx(),
            D::exif(),
        ]),
        Rule::PgYdotd => D::pgydotd(),
        Rule::IsoWeekDay => D::isoweekday(),
        Rule::IsoWeek => D::isoweek(),
        Rule::PgTextShort => D::pgtextshort(),
        Rule::PgTextReverse => D::pgtextreverse(),
        Rule::Clf => D::clf(),
        Rule::Year4 => D::year4(),
        Rule::Ago => ilit("ago"),
        Rule::DayText => D::daytext(),
        Rule::RelativeTextWeek => D::relativetextweek(),
        Rule::RelativeText => D::relativetext(),
        Rule::MonthText => alt(vec![D::monthfull(), D::monthabbr()]),
        Rule::Tz => alt(vec![D::tzcorrection(), D::tz()]),
        Rule::DateShortWithTime12 => alt(vec![
            D::dateshortwithtimeshort12(),
            D::dateshortwithtimelong12(),
        ]),
        Rule::DateShortWithTime24 => alt(vec![
            D::dateshortwithtimeshort(),
            D::dateshortwithtimelong(),
            D::dateshortwithtimelongtz(),
        ]),
        Rule::Relative => D::relative(),
        Rule::Separator => class(".,"),
        Rule::Space => D::space(),
        Rule::NulNewline => alt(vec![byte(0), lit("\n")]),
        Rule::Any => any(),
    }
}

/// The compiled strtotime scanner.
pub(crate) fn scanner() -> &'static Scanner {
    static SCANNER: OnceLock<Scanner> = OnceLock::new();
    SCANNER.get_or_init(|| {
        let res: Vec<Re> = RULES.iter().map(|&r| rule_re(r)).collect();
        Scanner::new(&res)
    })
}

/// Matches the longest token at `pos`.
pub(crate) fn next_token(buf: &[u8], pos: usize) -> (usize, Rule) {
    match scanner().longest_match(buf, pos) {
        Some((end, idx)) => (end, RULES[idx]),
        // `any` always matches one byte, so this only happens at the very end.
        None => (pos + 1, Rule::Any),
    }
}
